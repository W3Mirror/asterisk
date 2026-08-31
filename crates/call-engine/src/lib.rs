//! Provider-neutral SIP call orchestration.
//!
//! [`CallEngine`] composes the already-separated parser/transport, transaction,
//! dialog, and call-control layers.  It deliberately accepts parsed SIP
//! messages and returns outbound messages as actions; socket ownership and an
//! async runtime remain outside this crate.  This makes the call path
//! deterministic in tests and keeps the Asterisk fallback untouched.

use std::{
    borrow::Cow,
    collections::HashMap,
    error::Error,
    fmt::{Display, Formatter},
    net::SocketAddr,
    time::Duration,
};

use call_api::{
    ApiError, CallCommand, CallRegistry, CallRegistryConfig, CallSnapshot, NegotiatedAudio,
};
use call_core::{CallId, CallState, LifecycleEvent};
use sip_auth::{DigestAuthorization, DigestChallenge, DigestCredentials, DigestError};
use sip_dialog::{Dialog, DialogConfig, DialogError, DialogState};
use sip_transaction::{
    ClientAction, ClientState, ClientTransaction, ServerAction, ServerState, ServerTransaction,
    TimerConfig, TransactionError, TransportReliability,
};
use sip_types::{Headers, SipMessage, SipMethod, SipRequest, SipResponse};

const DEFAULT_MAX_TRANSACTIONS: usize = 8_192;
const DEFAULT_MAX_BRANCH_BYTES: usize = 256;
const DEFAULT_MAX_DIGEST_RETRIES_PER_CALL: usize = 2;

/// Bounds and protocol settings for a [`CallEngine`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EngineConfig {
    /// Bounds for application call records and lifecycle events.
    pub call_registry: CallRegistryConfig,
    /// Bounds used while constructing and validating SIP dialogs.
    pub dialog: DialogConfig,
    /// Deterministic transaction timer values.
    pub timers: TimerConfig,
    /// Maximum number of live client and server transactions combined.
    pub max_transactions: usize,
    /// Maximum authenticated INVITE retries accepted for one outbound call.
    pub max_digest_retries_per_call: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            call_registry: CallRegistryConfig::default(),
            dialog: DialogConfig::default(),
            timers: TimerConfig::default(),
            max_transactions: DEFAULT_MAX_TRANSACTIONS,
            max_digest_retries_per_call: DEFAULT_MAX_DIGEST_RETRIES_PER_CALL,
        }
    }
}

impl EngineConfig {
    fn validate(self) -> Result<Self, EngineError> {
        if self.max_transactions == 0
            || self.max_digest_retries_per_call == 0
            || self.dialog.max_field_bytes == 0
            || self.dialog.max_route_entries == 0
            || self.timers.t1.is_zero()
            || self.timers.t2 < self.timers.t1
            || self.timers.t4.is_zero()
        {
            return Err(EngineError::InvalidConfig);
        }
        Ok(self)
    }
}

/// Errors raised while composing SIP transactions, dialogs, and calls.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EngineError {
    /// A configured engine bound was zero.
    InvalidConfig,
    /// The transaction bound was reached.
    TransactionLimitReached,
    /// A required SIP header was absent.
    MissingHeader(&'static str),
    /// A Via header did not contain a usable transaction branch.
    InvalidBranch,
    /// A CSeq header did not contain a usable method.
    InvalidCSeq,
    /// A transaction branch is already active.
    DuplicateTransaction,
    /// No active transaction matched a SIP response.
    UnknownTransaction,
    /// No call/dialog matched the SIP Call-ID.
    UnknownDialog,
    /// The request is outside the provider-neutral basic-call surface.
    UnsupportedRequest,
    /// The requested call is not an inbound INVITE transaction.
    NotInboundInvite,
    /// The supplied status code cannot be used for the requested operation.
    InvalidResponseStatus,
    /// The outbound call reached its configured Digest retry bound.
    DigestRetryLimitReached,
    /// A PRACK did not match a reliable provisional response.
    InvalidPrack,
    /// A PRACK was received without an outstanding reliable provisional response.
    PrackNotExpected,
    /// A reliable provisional response is still awaiting its PRACK.
    ReliableProvisionalOutstanding,
    /// The provider policy does not configure Digest authentication.
    DigestAuthenticationNotConfigured,
    /// The current provider Digest credentials could not be resolved.
    DigestCredentialsUnavailable,
    /// A SIP Digest challenge or authorization could not be processed.
    Digest(DigestError),
    /// The call-control registry rejected an operation.
    CallApi(ApiError),
    /// The SIP dialog rejected a message or transition.
    Dialog(DialogError),
    /// The SIP transaction rejected a message or transition.
    Transaction(TransactionError),
}

impl Display for EngineError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfig => formatter.write_str("call engine bounds must be non-zero"),
            Self::TransactionLimitReached => {
                formatter.write_str("call engine reached its transaction limit")
            }
            Self::MissingHeader(name) => write!(formatter, "SIP message requires {name}"),
            Self::InvalidBranch => formatter.write_str("SIP Via does not contain a valid branch"),
            Self::InvalidCSeq => formatter.write_str("SIP CSeq does not contain a valid method"),
            Self::DuplicateTransaction => {
                formatter.write_str("SIP transaction branch is already active")
            }
            Self::UnknownTransaction => formatter.write_str("SIP transaction is not registered"),
            Self::UnknownDialog => formatter.write_str("SIP dialog is not registered"),
            Self::UnsupportedRequest => {
                formatter.write_str("SIP request is outside the basic call surface")
            }
            Self::NotInboundInvite => {
                formatter.write_str("call is not backed by an inbound INVITE")
            }
            Self::InvalidResponseStatus => {
                formatter.write_str("response status is invalid for this operation")
            }
            Self::DigestRetryLimitReached => {
                formatter.write_str("outbound call reached its SIP Digest retry limit")
            }
            Self::InvalidPrack => {
                formatter.write_str("SIP PRACK does not match its provisional response")
            }
            Self::PrackNotExpected => {
                formatter.write_str("SIP PRACK has no outstanding reliable provisional response")
            }
            Self::ReliableProvisionalOutstanding => {
                formatter.write_str("SIP reliable provisional response is still awaiting PRACK")
            }
            Self::DigestAuthenticationNotConfigured => {
                formatter.write_str("provider SIP Digest authentication is not configured")
            }
            Self::DigestCredentialsUnavailable => {
                formatter.write_str("provider SIP Digest credentials are unavailable")
            }
            Self::Digest(error) => Display::fmt(error, formatter),
            Self::CallApi(error) => Display::fmt(error, formatter),
            Self::Dialog(error) => Display::fmt(error, formatter),
            Self::Transaction(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for EngineError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CallApi(error) => Some(error),
            Self::Dialog(error) => Some(error),
            Self::Transaction(error) => Some(error),
            Self::Digest(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ApiError> for EngineError {
    fn from(error: ApiError) -> Self {
        Self::CallApi(error)
    }
}

impl From<DialogError> for EngineError {
    fn from(error: DialogError) -> Self {
        Self::Dialog(error)
    }
}

impl From<TransactionError> for EngineError {
    fn from(error: TransactionError) -> Self {
        Self::Transaction(error)
    }
}

impl From<DigestError> for EngineError {
    fn from(error: DigestError) -> Self {
        Self::Digest(error)
    }
}

/// A message that the outer transport adapter should send.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SendAction {
    /// Destination selected by the application/transport boundary.
    pub destination: SocketAddr,
    /// Parsed SIP message to serialize and send.
    pub message: SipMessage,
}

/// Deterministic result of one engine operation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EngineOutput {
    actions: Vec<SendAction>,
    events: Vec<LifecycleEvent>,
}

impl EngineOutput {
    /// Returns outbound messages in emission order.
    #[must_use]
    pub fn actions(&self) -> &[SendAction] {
        &self.actions
    }

    /// Returns lifecycle events emitted by this operation in order.
    #[must_use]
    pub fn events(&self) -> &[LifecycleEvent] {
        &self.events
    }

    /// Consumes the output and returns its outbound actions.
    #[must_use]
    pub fn into_actions(self) -> Vec<SendAction> {
        self.actions
    }

    /// Consumes the output and returns its lifecycle events.
    #[must_use]
    pub fn into_events(self) -> Vec<LifecycleEvent> {
        self.events
    }

    /// Consumes the output and returns actions and lifecycle events together.
    #[must_use]
    pub fn into_parts(self) -> (Vec<SendAction>, Vec<LifecycleEvent>) {
        (self.actions, self.events)
    }
}

/// A bounded, runtime-agnostic SIP call orchestrator.
#[derive(Clone, Debug)]
pub struct CallEngine {
    config: EngineConfig,
    registry: CallRegistry,
    dialogs: HashMap<CallId, Dialog>,
    client_transactions: HashMap<String, ClientTransaction>,
    client_destinations: HashMap<String, SocketAddr>,
    client_calls: HashMap<String, CallId>,
    server_transactions: HashMap<String, ServerTransaction>,
    server_destinations: HashMap<String, SocketAddr>,
    server_calls: HashMap<String, CallId>,
    final_invites: HashMap<String, FinalInvite>,
    final_server_invites: HashMap<String, FinalServerInvite>,
    digest_retries: HashMap<CallId, usize>,
    received_reliable_rseq: HashMap<CallId, u32>,
    reliable_provisionals: HashMap<CallId, ReliableProvisional>,
    next_rseq: u32,
    next_local_tag: u64,
    next_branch: u64,
}

#[derive(Clone, Debug)]
struct ReliableProvisional {
    rseq: u32,
    invite_sequence: u32,
}

#[derive(Clone, Debug)]
struct FinalInvite {
    request: SipRequest,
    response: SipResponse,
    destination: SocketAddr,
    call_id: CallId,
    dialog: Dialog,
    successful: bool,
}

#[derive(Clone, Debug)]
struct FinalServerInvite {
    request: SipRequest,
    response: SipResponse,
    destination: SocketAddr,
    call_id: CallId,
    acknowledged: bool,
    next_retransmit: Option<Duration>,
    retransmit_interval: Duration,
}

impl CallEngine {
    /// Creates an empty engine with validated resource bounds.
    pub fn new(config: EngineConfig) -> Result<Self, EngineError> {
        let config = config.validate()?;
        Ok(Self {
            registry: CallRegistry::new(config.call_registry)?,
            config,
            dialogs: HashMap::new(),
            client_transactions: HashMap::new(),
            client_destinations: HashMap::new(),
            client_calls: HashMap::new(),
            server_transactions: HashMap::new(),
            server_destinations: HashMap::new(),
            server_calls: HashMap::new(),
            final_invites: HashMap::new(),
            final_server_invites: HashMap::new(),
            digest_retries: HashMap::new(),
            received_reliable_rseq: HashMap::new(),
            reliable_provisionals: HashMap::new(),
            next_rseq: 1,
            next_local_tag: 1,
            next_branch: 1,
        })
    }

    /// Returns the engine's immutable configuration.
    #[must_use]
    pub fn config(&self) -> EngineConfig {
        self.config
    }

    /// Returns the number of active client and server transactions.
    #[must_use]
    pub fn transaction_count(&self) -> usize {
        self.client_transactions.len() + self.server_transactions.len()
    }

    /// Returns the number of retained SIP dialogs.
    #[must_use]
    pub fn dialog_count(&self) -> usize {
        self.dialogs.len()
    }

    /// Returns a stable call snapshot.
    pub fn snapshot(&self, id: &CallId) -> Result<CallSnapshot, EngineError> {
        Ok(self.registry.snapshot(id)?)
    }

    /// Returns deterministic call snapshots ordered by application ID.
    pub fn list(&self, limit: usize) -> Result<Vec<CallSnapshot>, EngineError> {
        Ok(self.registry.list(limit)?)
    }

    /// Applies an application call-control command and returns its events.
    pub fn apply_call_command(
        &mut self,
        id: &CallId,
        command: CallCommand,
    ) -> Result<EngineOutput, EngineError> {
        self.registry.apply(id, command)?;
        if matches!(
            self.registry.snapshot(id)?.state,
            CallState::Ended | CallState::Failed
        ) {
            self.remove_final_invites_for_call(id);
            self.remove_final_server_invites_for_call(id);
            self.remove_reliable_provisional_state(id);
        }
        self.finish(Vec::new())
    }

    /// Removes a terminal call and every retained signaling resource owned by it.
    ///
    /// # Errors
    ///
    /// Returns an error when the call is unknown or has not reached `Ended` or
    /// `Failed`. Rejected reclamation leaves the engine unchanged.
    pub fn reclaim_terminal_call(&mut self, id: &CallId) -> Result<CallSnapshot, EngineError> {
        let snapshot = self.registry.remove_terminal(id)?;
        self.dialogs.remove(id);

        let client_branches = self
            .client_calls
            .iter()
            .filter(|(_, call_id)| *call_id == id)
            .map(|(branch, _)| branch.clone())
            .collect::<Vec<_>>();
        for branch in client_branches {
            self.remove_client_transaction(&branch);
        }

        let server_branches = self
            .server_calls
            .iter()
            .filter(|(_, call_id)| *call_id == id)
            .map(|(branch, _)| branch.clone())
            .collect::<Vec<_>>();
        for branch in server_branches {
            self.remove_server_transaction(&branch);
        }

        self.remove_final_invites_for_call(id);
        self.remove_final_server_invites_for_call(id);
        self.digest_retries.remove(id);
        self.remove_reliable_provisional_state(id);
        Ok(snapshot)
    }

    /// Negotiates and retains audio for a registered call.
    pub fn negotiate_audio(
        &mut self,
        id: &CallId,
        local: &sdp::SessionDescription,
        remote: &sdp::SessionDescription,
    ) -> Result<NegotiatedAudio, EngineError> {
        Ok(self.registry.negotiate_audio(id, local, remote)?)
    }

    /// Starts an outbound INVITE transaction and creates its UAC dialog.
    ///
    /// The request must already contain normal SIP identity headers (including
    /// a From tag, Call-ID, CSeq, and Via branch).  Generating provider-
    /// specific identities remains an application concern.
    pub fn originate(
        &mut self,
        request: SipRequest,
        destination: SocketAddr,
        now: Duration,
        reliability: TransportReliability,
    ) -> Result<(CallId, EngineOutput), EngineError> {
        if request.method != SipMethod::Invite {
            return Err(EngineError::UnsupportedRequest);
        }
        self.ensure_transaction_capacity()?;
        let branch = transaction_branch(&request.headers)?;
        let key = transaction_key(&branch, &request.method);
        self.ensure_new_transaction(&key)?;
        let dialog = Dialog::from_uac_request(&request, self.config.dialog)?;
        let transaction =
            ClientTransaction::new(request.clone(), now, reliability, self.config.timers)?;
        self.ensure_event_capacity(2)?;

        let id = self.registry.create()?;
        self.registry.apply(&id, CallCommand::InviteReceived)?;
        self.registry.bind_dialog(&id, dialog.id())?;
        self.dialogs.insert(id.clone(), dialog);
        self.client_transactions.insert(key.clone(), transaction);
        self.client_destinations.insert(key.clone(), destination);
        self.client_calls.insert(key, id.clone());

        let output = self.finish(vec![SendAction {
            destination,
            message: SipMessage::Request(request),
        }])?;
        Ok((id, output))
    }

    /// Receives an inbound SIP request from a parsed UDP/TCP transport.
    ///
    /// Initial INVITEs create a UAS dialog and receive an automatic `100
    /// Trying`.  The application can then call [`Self::respond_to_invite`] to
    /// ring, answer, or reject the call.  ACK for a successful final response
    /// is handled as an in-dialog request and produces no response.
    pub fn receive_request(
        &mut self,
        source: SocketAddr,
        request: SipRequest,
        now: Duration,
        reliability: TransportReliability,
    ) -> Result<EngineOutput, EngineError> {
        let branch = transaction_branch(&request.headers)?;
        validate_request_cseq(&request.headers, &request.method)?;
        let request_key = transaction_key(&branch, &request.method);
        let lookup_key = if self.server_transactions.contains_key(&request_key) {
            request_key.clone()
        } else if request.method == SipMethod::Ack {
            transaction_key(&branch, &SipMethod::Invite)
        } else {
            request_key.clone()
        };

        if request.method == SipMethod::Invite {
            if let Some(final_invite) = self.final_server_invites.get(&request_key) {
                if header_value(&request.headers, "Call-ID")
                    != header_value(&final_invite.request.headers, "Call-ID")
                    || header_value(&request.headers, "CSeq")
                        != header_value(&final_invite.request.headers, "CSeq")
                {
                    return Err(EngineError::UnknownTransaction);
                }
                return self.finish(vec![SendAction {
                    destination: final_invite.destination,
                    message: SipMessage::Response(final_invite.response.clone()),
                }]);
            }
        }

        // Retransmissions belong to the existing server transaction and must
        // replay its last response without creating another call.
        if let Some(transaction) = self.server_transactions.get_mut(&lookup_key) {
            let actions = transaction.on_request(&request, now)?;
            let mut output_actions = Vec::new();
            if actions
                .iter()
                .any(|action| matches!(action, ServerAction::RetransmitResponse))
            {
                if let Some(response) = transaction.last_response().cloned() {
                    let destination = self
                        .server_destinations
                        .get(&lookup_key)
                        .copied()
                        .ok_or(EngineError::UnknownTransaction)?;
                    output_actions.push(SendAction {
                        destination,
                        message: SipMessage::Response(response),
                    });
                }
            }
            return self.finish(output_actions);
        }

        if request.method == SipMethod::Options && !request_has_to_tag(&request) {
            return self.receive_options(source, request, request_key, now, reliability);
        }

        if !matches!(
            request.method,
            SipMethod::Invite
                | SipMethod::Ack
                | SipMethod::Bye
                | SipMethod::Cancel
                | SipMethod::Prack
                | SipMethod::Options
        ) {
            return Err(EngineError::UnsupportedRequest);
        }

        if request.method == SipMethod::Cancel {
            return self.receive_cancel(source, request, branch, now, reliability);
        }

        if request.method == SipMethod::Prack {
            return self.receive_prack(source, request, branch, now, reliability);
        }

        if request.method == SipMethod::Ack {
            return self.receive_ack(request);
        }

        if request.method == SipMethod::Invite && !request_has_to_tag(&request) {
            return self.receive_initial_invite(source, request, branch, reliability);
        }

        self.receive_in_dialog(source, request, branch, now, reliability)
    }

    /// Receives a response for an outbound client transaction.
    pub fn receive_response(
        &mut self,
        response: SipResponse,
        now: Duration,
    ) -> Result<EngineOutput, EngineError> {
        let branch = transaction_branch(&response.headers)?;
        let response_method = cseq_method(&response.headers)?;
        let key = transaction_key(&branch, &response_method);
        if let Some(final_invite) = self.final_invites.get(&key).cloned() {
            if response.status_code < 200
                || (response.status_code < 300) != final_invite.successful
                || required_header(&response.headers, "Call-ID")?
                    != required_header(&final_invite.response.headers, "Call-ID")?
                || required_header(&response.headers, "CSeq")?
                    != required_header(&final_invite.response.headers, "CSeq")?
            {
                return Err(EngineError::UnknownTransaction);
            }
            let mut next_branch = self.next_branch;
            let ack = build_ack(
                &mut next_branch,
                &final_invite.request,
                &response,
                &final_invite.dialog,
                final_invite.successful,
            )?;
            self.next_branch = next_branch;
            return self.finish(vec![SendAction {
                destination: final_invite.destination,
                message: SipMessage::Request(ack),
            }]);
        }

        let mut transaction = self
            .client_transactions
            .get(&key)
            .cloned()
            .ok_or(EngineError::UnknownTransaction)?;
        let destination = self
            .client_destinations
            .get(&key)
            .copied()
            .ok_or(EngineError::UnknownTransaction)?;
        let call_id = self
            .client_calls
            .get(&key)
            .cloned()
            .ok_or(EngineError::UnknownTransaction)?;
        let mut dialog = self
            .dialogs
            .get(&call_id)
            .cloned()
            .ok_or(EngineError::UnknownDialog)?;
        let mut registry = self.registry.clone();
        let mut next_branch = self.next_branch;
        let request = transaction.request().clone();
        let reliable_rseq = reliable_provisional_rseq(&response)?;

        let _transaction_actions = transaction.on_response(&response, now)?;
        let call_state = registry.snapshot(&call_id)?.state;
        let command = (response_method == SipMethod::Invite)
            .then(|| response_call_command(call_state, response.status_code))
            .flatten();
        if command.is_some() {
            ensure_event_capacity_for(&registry, 1)?;
        }
        let _dialog_actions = dialog.receive_response(&response)?;
        if let Some(command) = command {
            registry.apply(&call_id, command)?;
        }

        let mut output_actions = Vec::new();
        let prack = if response_method == SipMethod::Invite {
            reliable_rseq.and_then(|rseq| {
                (self.received_reliable_rseq.get(&call_id).copied() != Some(rseq)).then_some(rseq)
            })
        } else {
            None
        };
        let prack = if let Some(rseq) = prack {
            if self
                .received_reliable_rseq
                .get(&call_id)
                .is_some_and(|previous| rseq < *previous)
            {
                return Err(EngineError::InvalidPrack);
            }
            self.ensure_transaction_capacity()?;
            let invite_sequence = invite_cseq(&request.headers)?;
            let prack = build_prack(
                &mut next_branch,
                &request,
                &response,
                &mut dialog,
                rseq,
                invite_sequence,
            )?;
            let prack_key =
                transaction_key(&transaction_branch(&prack.headers)?, &SipMethod::Prack);
            self.ensure_new_transaction(&prack_key)?;
            let prack_transaction = ClientTransaction::new(
                prack.clone(),
                now,
                transaction.reliability(),
                self.config.timers,
            )?;
            Some((rseq, prack_key, prack, prack_transaction))
        } else {
            None
        };
        let final_invite = if response_method == SipMethod::Invite && response.status_code >= 200 {
            let successful = response.status_code < 300;
            let ack = build_ack(&mut next_branch, &request, &response, &dialog, successful)?;
            output_actions.push(SendAction {
                destination,
                message: SipMessage::Request(ack),
            });
            if !successful {
                registry.apply(&call_id, CallCommand::End)?;
            }
            Some(FinalInvite {
                request,
                response: response.clone(),
                destination,
                call_id: call_id.clone(),
                dialog: dialog.clone(),
                successful,
            })
        } else {
            None
        };

        self.registry = registry;
        self.next_branch = next_branch;
        if response.status_code >= 300 {
            self.dialogs.remove(&call_id);
        } else {
            self.dialogs.insert(call_id.clone(), dialog);
        }
        if response_method == SipMethod::Invite && response.status_code >= 200 {
            self.received_reliable_rseq.remove(&call_id);
        }

        // A non-2xx INVITE client transaction remains in Completed until Timer
        // D; a successful INVITE transaction is complete immediately.
        if !matches!(transaction.state(), ClientState::Terminated) {
            self.client_transactions.insert(key.clone(), transaction);
        } else {
            self.client_transactions.remove(&key);
            self.client_destinations.remove(&key);
            self.client_calls.remove(&key);
        }
        if let Some(final_invite) = final_invite {
            self.final_invites.insert(key, final_invite);
        }
        if let Some((rseq, prack_key, prack, prack_transaction)) = prack {
            self.received_reliable_rseq.insert(call_id.clone(), rseq);
            self.client_transactions
                .insert(prack_key.clone(), prack_transaction);
            self.client_destinations
                .insert(prack_key.clone(), destination);
            self.client_calls.insert(prack_key, call_id);
            output_actions.push(SendAction {
                destination,
                message: SipMessage::Request(prack),
            });
        }

        self.finish(output_actions)
    }

    /// Receives an outbound INVITE authentication challenge and starts one
    /// bounded authenticated retry without ending the logical call.
    ///
    /// The caller supplies credentials and qop inputs explicitly; the engine
    /// does not retain either. A duplicate challenge retransmission only
    /// replays its transaction ACK and never consumes another retry.
    ///
    /// # Errors
    ///
    /// Returns an error when the response is not a matching 401/407 INVITE
    /// challenge, Digest construction fails, a resource bound is reached, or
    /// the replacement transaction cannot be built. Every error leaves the
    /// engine unchanged.
    pub fn receive_digest_challenge(
        &mut self,
        response: SipResponse,
        now: Duration,
        credentials: &DigestCredentials,
        cnonce: Option<&str>,
        nonce_count: Option<u32>,
    ) -> Result<EngineOutput, EngineError> {
        self.receive_digest_challenge_with(response, now, cnonce, nonce_count, || {
            Ok(Cow::Borrowed(credentials))
        })
    }

    /// Receives an outbound INVITE authentication challenge and lazily
    /// resolves credentials only when a new authenticated retry is required.
    ///
    /// The resolver is not called for a duplicate challenge that only needs
    /// its transaction ACK replayed. Any owned credentials it returns are
    /// dropped after this operation and are never retained by the engine.
    ///
    /// # Errors
    ///
    /// Returns an error when challenge processing or credential resolution
    /// fails. Every error leaves the engine unchanged.
    pub fn receive_digest_challenge_resolved<F>(
        &mut self,
        response: SipResponse,
        now: Duration,
        cnonce: Option<&str>,
        nonce_count: Option<u32>,
        resolve: F,
    ) -> Result<EngineOutput, EngineError>
    where
        F: FnOnce() -> Result<DigestCredentials, EngineError>,
    {
        self.receive_digest_challenge_with(response, now, cnonce, nonce_count, || {
            resolve().map(Cow::Owned)
        })
    }

    fn receive_digest_challenge_with<'a, F>(
        &mut self,
        response: SipResponse,
        now: Duration,
        cnonce: Option<&str>,
        nonce_count: Option<u32>,
        resolve: F,
    ) -> Result<EngineOutput, EngineError>
    where
        F: FnOnce() -> Result<Cow<'a, DigestCredentials>, EngineError>,
    {
        let mut working = self.clone();
        let output =
            working.receive_digest_challenge_inner(response, now, cnonce, nonce_count, resolve)?;
        *self = working;
        Ok(output)
    }

    fn receive_digest_challenge_inner<'a, F>(
        &mut self,
        response: SipResponse,
        now: Duration,
        cnonce: Option<&str>,
        nonce_count: Option<u32>,
        resolve: F,
    ) -> Result<EngineOutput, EngineError>
    where
        F: FnOnce() -> Result<Cow<'a, DigestCredentials>, EngineError>,
    {
        let (challenge_header, authorization_header) = digest_header_names(response.status_code)?;
        let key = invite_response_key(&response)?;

        if let Some(final_invite) = self.final_invites.get(&key).cloned() {
            return self.replay_digest_challenge_ack(&final_invite, &response);
        }

        let mut transaction = self
            .client_transactions
            .get(&key)
            .cloned()
            .ok_or(EngineError::UnknownTransaction)?;
        let request = transaction.request().clone();
        if request.method != SipMethod::Invite {
            return Err(EngineError::UnsupportedRequest);
        }
        let destination = self
            .client_destinations
            .get(&key)
            .copied()
            .ok_or(EngineError::UnknownTransaction)?;
        let call_id = self
            .client_calls
            .get(&key)
            .cloned()
            .ok_or(EngineError::UnknownTransaction)?;
        let dialog = self
            .dialogs
            .get(&call_id)
            .cloned()
            .ok_or(EngineError::UnknownDialog)?;
        let retry_count = self.available_digest_retry_count(&call_id)?;

        let challenge =
            DigestChallenge::parse(required_header(&response.headers, challenge_header)?)?;
        let mut validation_dialog = dialog.clone();
        let _ = validation_dialog.receive_response(&response)?;
        let _ = transaction.on_response(&response, now)?;

        if !matches!(transaction.state(), ClientState::Terminated)
            && self.transaction_count() >= self.config.max_transactions
        {
            return Err(EngineError::TransactionLimitReached);
        }

        let mut next_branch = self.next_branch;
        let retry_branch = self.allocate_unique_client_branch(&mut next_branch)?;
        let credentials = resolve()?;
        let retry_request = build_digest_retry_request(
            &request,
            &retry_branch,
            authorization_header,
            &challenge,
            credentials.as_ref(),
            cnonce,
            nonce_count,
        )?;
        let retry_key = transaction_key(&retry_branch, &SipMethod::Invite);
        self.ensure_new_transaction(&retry_key)?;
        let retry_dialog = Dialog::from_uac_request(&retry_request, self.config.dialog)?;
        let retry_transaction = ClientTransaction::new(
            retry_request.clone(),
            now,
            transaction.reliability(),
            self.config.timers,
        )?;
        let ack = build_ack(&mut next_branch, &request, &response, &dialog, false)?;
        let next_retry_count = retry_count
            .checked_add(1)
            .ok_or(EngineError::DigestRetryLimitReached)?;

        if matches!(transaction.state(), ClientState::Terminated) {
            self.remove_client_transaction(&key);
        } else {
            self.client_transactions.insert(key.clone(), transaction);
        }
        self.final_invites.insert(
            key,
            FinalInvite {
                request,
                response,
                destination,
                call_id: call_id.clone(),
                dialog,
                successful: false,
            },
        );
        self.client_transactions
            .insert(retry_key.clone(), retry_transaction);
        self.client_destinations
            .insert(retry_key.clone(), destination);
        self.client_calls.insert(retry_key, call_id.clone());
        self.dialogs.insert(call_id.clone(), retry_dialog);
        self.digest_retries.insert(call_id, next_retry_count);
        self.next_branch = next_branch;

        self.finish(vec![
            SendAction {
                destination,
                message: SipMessage::Request(ack),
            },
            SendAction {
                destination,
                message: SipMessage::Request(retry_request),
            },
        ])
    }

    fn replay_digest_challenge_ack(
        &mut self,
        final_invite: &FinalInvite,
        response: &SipResponse,
    ) -> Result<EngineOutput, EngineError> {
        if final_invite.successful
            || final_invite.response.status_code != response.status_code
            || required_header(&response.headers, "Call-ID")?
                != required_header(&final_invite.response.headers, "Call-ID")?
            || required_header(&response.headers, "CSeq")?
                != required_header(&final_invite.response.headers, "CSeq")?
        {
            return Err(EngineError::UnknownTransaction);
        }
        let mut next_branch = self.next_branch;
        let ack = build_ack(
            &mut next_branch,
            &final_invite.request,
            response,
            &final_invite.dialog,
            false,
        )?;
        self.next_branch = next_branch;
        self.finish(vec![SendAction {
            destination: final_invite.destination,
            message: SipMessage::Request(ack),
        }])
    }

    fn available_digest_retry_count(&self, call_id: &CallId) -> Result<usize, EngineError> {
        let retry_count = self.digest_retries.get(call_id).copied().unwrap_or(0);
        if retry_count >= self.config.max_digest_retries_per_call {
            Err(EngineError::DigestRetryLimitReached)
        } else {
            Ok(retry_count)
        }
    }

    /// Sends a provisional or final response to an inbound INVITE.
    pub fn respond_to_invite(
        &mut self,
        id: &CallId,
        status_code: u16,
        reason: impl Into<String>,
        body: Vec<u8>,
        now: Duration,
    ) -> Result<EngineOutput, EngineError> {
        if !(100..=699).contains(&status_code) || status_code < 180 {
            return Err(EngineError::InvalidResponseStatus);
        }
        let (branch, destination) = self
            .server_calls
            .iter()
            .find_map(|(key, call)| {
                (call == id
                    && self
                        .server_transactions
                        .get(key)
                        .is_some_and(|transaction| {
                            transaction.request().method == SipMethod::Invite
                        }))
                .then_some((key.clone(), self.server_destinations.get(key).copied()))
            })
            .and_then(|(branch, destination)| destination.map(|destination| (branch, destination)))
            .ok_or(EngineError::NotInboundInvite)?;
        let request = self
            .server_transactions
            .get(&branch)
            .ok_or(EngineError::NotInboundInvite)?
            .request()
            .clone();
        if request.method != SipMethod::Invite {
            return Err(EngineError::NotInboundInvite);
        }
        let local_tag = self
            .dialogs
            .get(id)
            .ok_or(EngineError::UnknownDialog)?
            .local_tag()
            .to_owned();
        let reason = reason.into();
        let mut response =
            response_for_request(&request, status_code, &reason, Some(&local_tag), body)?;
        if !response.body.is_empty() {
            response.headers.push("Content-Type", "application/sdp");
        }
        let reliable = status_code > 100
            && status_code < 200
            && header_has_token(&request.headers, "Require", "100rel");
        if reliable && self.reliable_provisionals.contains_key(id) {
            return Err(EngineError::ReliableProvisionalOutstanding);
        }
        let mut next_rseq = self.next_rseq;
        let reliable_provisional = if reliable {
            let rseq = next_rseq;
            next_rseq = next_rseq.checked_add(1).ok_or(EngineError::InvalidConfig)?;
            response.headers.push("Require", "100rel");
            response.headers.push("RSeq", rseq.to_string());
            Some(ReliableProvisional {
                rseq,
                invite_sequence: invite_cseq(&request.headers)?,
            })
        } else {
            None
        };

        let mut registry = self.registry.clone();
        let call_state = registry.snapshot(id)?.state;
        let command = response_call_command(call_state, status_code);
        if command.is_some() {
            ensure_event_capacity_for(&registry, 1)?;
        }
        let mut transaction = self
            .server_transactions
            .get(&branch)
            .cloned()
            .ok_or(EngineError::NotInboundInvite)?;
        let transaction_reliability = transaction.reliability();
        let actions = if status_code < 200 {
            transaction.send_provisional(response.clone())?
        } else {
            transaction.send_final(response.clone(), now)?
        };
        if let Some(command) = command {
            registry.apply(id, command)?;
        }
        if status_code >= 300 {
            registry.apply(id, CallCommand::End)?;
        }
        let final_server_invite = (status_code < 300).then(|| FinalServerInvite {
            request: request.clone(),
            response: response.clone(),
            destination,
            call_id: id.clone(),
            acknowledged: false,
            next_retransmit: (transaction_reliability == TransportReliability::Unreliable)
                .then(|| now.saturating_add(self.config.timers.t1)),
            retransmit_interval: self.config.timers.t1,
        });
        self.registry = registry;
        if actions
            .iter()
            .any(|action| matches!(action, ServerAction::Terminated))
        {
            self.remove_server_transaction(&branch);
        } else {
            self.server_transactions.insert(branch.clone(), transaction);
        }
        if status_code >= 300 {
            self.dialogs.remove(id);
        }
        self.next_rseq = next_rseq;
        if let Some(reliable_provisional) = reliable_provisional {
            self.reliable_provisionals
                .insert(id.clone(), reliable_provisional);
        } else if status_code >= 200 {
            self.reliable_provisionals.remove(id);
        }
        if let Some(final_invite) = final_server_invite {
            self.final_server_invites.insert(branch, final_invite);
        }
        self.finish(vec![SendAction {
            destination,
            message: SipMessage::Response(response),
        }])
    }

    /// Polls all transaction timers at a deterministic monotonic time.
    pub fn poll(&mut self, now: Duration) -> Result<EngineOutput, EngineError> {
        let mut working = self.clone();
        let output = working.poll_inner(now)?;
        *self = working;
        Ok(output)
    }

    fn poll_inner(&mut self, now: Duration) -> Result<EngineOutput, EngineError> {
        let client_branches = self.client_transactions.keys().cloned().collect::<Vec<_>>();
        let server_branches = self.server_transactions.keys().cloned().collect::<Vec<_>>();
        let mut output_actions = Vec::new();
        let mut timed_out_calls = Vec::new();

        for branch in client_branches {
            let Some(transaction) = self.client_transactions.get_mut(&branch) else {
                continue;
            };
            let actions = transaction.poll(now);
            let destination = self.client_destinations.get(&branch).copied();
            for action in actions {
                match action {
                    ClientAction::RetransmitRequest => {
                        if let Some(destination) = destination {
                            output_actions.push(SendAction {
                                destination,
                                message: SipMessage::Request(transaction.request().clone()),
                            });
                        }
                    }
                    ClientAction::TimedOut => {
                        if let Some(call_id) = self.client_calls.get(&branch).cloned() {
                            timed_out_calls.push(call_id);
                        }
                    }
                    ClientAction::StateChanged { .. }
                    | ClientAction::AckRequired
                    | ClientAction::Terminated => {}
                }
            }
            if transaction.state() == ClientState::Terminated {
                self.remove_client_transaction(&branch);
                if self
                    .final_invites
                    .get(&branch)
                    .is_some_and(|invite| !invite.successful)
                {
                    self.final_invites.remove(&branch);
                }
            }
        }

        for branch in server_branches {
            let Some(transaction) = self.server_transactions.get_mut(&branch) else {
                continue;
            };
            let actions = transaction.poll(now);
            let destination = self.server_destinations.get(&branch).copied();
            for action in actions {
                match action {
                    ServerAction::RetransmitResponse => {
                        if let (Some(destination), Some(response)) =
                            (destination, transaction.last_response().cloned())
                        {
                            output_actions.push(SendAction {
                                destination,
                                message: SipMessage::Response(response),
                            });
                        }
                    }
                    ServerAction::TimedOut => {
                        if let Some(call_id) = self.server_calls.get(&branch).cloned() {
                            timed_out_calls.push(call_id);
                        }
                    }
                    ServerAction::StateChanged { .. }
                    | ServerAction::AckAccepted
                    | ServerAction::Terminated => {}
                }
            }
            if transaction.state() == ServerState::Terminated {
                self.remove_server_transaction(&branch);
            }
        }

        let final_server_branches = self
            .final_server_invites
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for branch in final_server_branches {
            let Some(final_invite) = self.final_server_invites.get_mut(&branch) else {
                continue;
            };
            let Some(deadline) = final_invite.next_retransmit else {
                continue;
            };
            if final_invite.acknowledged || deadline > now {
                continue;
            }
            output_actions.push(SendAction {
                destination: final_invite.destination,
                message: SipMessage::Response(final_invite.response.clone()),
            });
            let interval = final_invite
                .retransmit_interval
                .checked_mul(2)
                .unwrap_or(Duration::MAX)
                .min(self.config.timers.t2);
            final_invite.retransmit_interval = interval;
            final_invite.next_retransmit = Some(now.saturating_add(interval));
        }

        timed_out_calls.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        timed_out_calls.dedup();
        for call_id in timed_out_calls {
            self.fail_call(&call_id)?;
            self.dialogs.remove(&call_id);
            self.remove_reliable_provisional_state(&call_id);
        }
        self.finish(output_actions)
    }

    fn receive_initial_invite(
        &mut self,
        source: SocketAddr,
        request: SipRequest,
        branch: String,
        reliability: TransportReliability,
    ) -> Result<EngineOutput, EngineError> {
        self.ensure_transaction_capacity()?;
        let key = transaction_key(&branch, &SipMethod::Invite);
        self.ensure_new_transaction(&key)?;
        self.ensure_event_capacity(2)?;
        let local_tag = self.allocate_local_tag()?;
        let dialog = Dialog::from_uas_invite(&request, &local_tag, self.config.dialog)?;
        let mut transaction =
            ServerTransaction::new(request.clone(), reliability, self.config.timers)?;
        let response = response_for_request(&request, 100, "Trying", Some(&local_tag), Vec::new())?;
        transaction.send_provisional(response.clone())?;

        let id = self.registry.create()?;
        self.registry.apply(&id, CallCommand::InviteReceived)?;
        self.registry.bind_dialog(&id, dialog.id())?;
        self.dialogs.insert(id.clone(), dialog);
        self.server_transactions.insert(key.clone(), transaction);
        self.server_destinations.insert(key.clone(), source);
        self.server_calls.insert(key, id);

        self.finish(vec![SendAction {
            destination: source,
            message: SipMessage::Response(response),
        }])
    }

    fn receive_options(
        &mut self,
        source: SocketAddr,
        request: SipRequest,
        key: String,
        now: Duration,
        reliability: TransportReliability,
    ) -> Result<EngineOutput, EngineError> {
        self.ensure_transaction_capacity()?;
        self.ensure_new_transaction(&key)?;
        let response = response_for_request(&request, 200, "OK", None, Vec::new())?;
        let mut transaction = ServerTransaction::new(request, reliability, self.config.timers)?;
        transaction.send_final(response.clone(), now)?;
        if transaction.state() != ServerState::Terminated {
            self.server_transactions.insert(key.clone(), transaction);
            self.server_destinations.insert(key, source);
        }
        self.finish(vec![SendAction {
            destination: source,
            message: SipMessage::Response(response),
        }])
    }

    fn receive_ack(&mut self, request: SipRequest) -> Result<EngineOutput, EngineError> {
        let sip_call_id = required_header(&request.headers, "Call-ID")?;
        let call_id = self
            .find_call_by_sip_call_id(sip_call_id)
            .ok_or(EngineError::UnknownDialog)?;
        let dialog = self
            .dialogs
            .get_mut(&call_id)
            .ok_or(EngineError::UnknownDialog)?;
        dialog.receive_request(&request)?;
        for final_invite in self.final_server_invites.values_mut() {
            if final_invite.call_id == call_id {
                final_invite.acknowledged = true;
                final_invite.next_retransmit = None;
            }
        }
        self.finish(Vec::new())
    }

    fn receive_cancel(
        &mut self,
        source: SocketAddr,
        request: SipRequest,
        branch: String,
        now: Duration,
        reliability: TransportReliability,
    ) -> Result<EngineOutput, EngineError> {
        let sip_call_id = required_header(&request.headers, "Call-ID")?.to_owned();
        let invite_key = transaction_key(&branch, &SipMethod::Invite);
        let Some(invite_transaction) = self.server_transactions.get(&invite_key) else {
            let response = response_for_request(
                &request,
                481,
                "Call/Transaction Does Not Exist",
                None,
                Vec::new(),
            )?;
            return self.finish(vec![SendAction {
                destination: source,
                message: SipMessage::Response(response),
            }]);
        };
        if invite_transaction.state() != ServerState::Proceeding
            && invite_transaction.state() != ServerState::Trying
        {
            let response = response_for_request(
                &request,
                481,
                "Call/Transaction Does Not Exist",
                None,
                Vec::new(),
            )?;
            return self.finish(vec![SendAction {
                destination: source,
                message: SipMessage::Response(response),
            }]);
        }
        let Some(call_id) = self.server_calls.get(&invite_key).cloned() else {
            return Err(EngineError::UnknownDialog);
        };
        if self
            .dialogs
            .get(&call_id)
            .is_none_or(|dialog| dialog.call_id() != sip_call_id)
        {
            return Err(EngineError::UnknownDialog);
        }
        self.ensure_transaction_capacity()?;
        let cancel_key = transaction_key(&branch, &SipMethod::Cancel);
        self.ensure_new_transaction(&cancel_key)?;
        ensure_event_capacity_for(&self.registry, 1)?;
        let local_tag = self
            .dialogs
            .get(&call_id)
            .ok_or(EngineError::UnknownDialog)?
            .local_tag()
            .to_owned();
        let response = response_for_request(&request, 200, "OK", None, Vec::new())?;
        let mut cancel_transaction =
            ServerTransaction::new(request, reliability, self.config.timers)?;
        cancel_transaction.send_final(response.clone(), now)?;
        let cancel_terminated = cancel_transaction.state() == ServerState::Terminated;
        let invite_source = self
            .server_destinations
            .get(&invite_key)
            .copied()
            .ok_or(EngineError::UnknownTransaction)?;
        let mut invite_transaction = invite_transaction.clone();
        let invite_request = invite_transaction.request().clone();
        let invite_response = response_for_request(
            &invite_request,
            487,
            "Request Terminated",
            Some(&local_tag),
            Vec::new(),
        )?;
        invite_transaction.send_final(invite_response.clone(), now)?;

        let mut output_actions = vec![SendAction {
            destination: source,
            message: SipMessage::Response(response),
        }];
        output_actions.push(SendAction {
            destination: invite_source,
            message: SipMessage::Response(invite_response),
        });
        let mut registry = self.registry.clone();
        registry.apply(&call_id, CallCommand::Fail)?;
        registry.apply(&call_id, CallCommand::End)?;
        self.registry = registry;
        if !cancel_terminated {
            self.server_transactions
                .insert(cancel_key.clone(), cancel_transaction);
            self.server_destinations.insert(cancel_key.clone(), source);
            self.server_calls.insert(cancel_key, call_id.clone());
        }
        self.server_transactions
            .insert(invite_key.clone(), invite_transaction);
        self.dialogs.remove(&call_id);
        self.finish(output_actions)
    }

    fn receive_in_dialog(
        &mut self,
        source: SocketAddr,
        request: SipRequest,
        branch: String,
        now: Duration,
        reliability: TransportReliability,
    ) -> Result<EngineOutput, EngineError> {
        if request.method == SipMethod::Prack {
            return self.receive_prack(source, request, branch, now, reliability);
        }
        let sip_call_id = required_header(&request.headers, "Call-ID")?.to_owned();
        let call_id = self
            .find_call_by_sip_call_id(&sip_call_id)
            .ok_or(EngineError::UnknownDialog)?;
        let key = transaction_key(&branch, &request.method);
        self.ensure_transaction_capacity()?;
        self.ensure_new_transaction(&key)?;
        if request.method == SipMethod::Bye {
            ensure_event_capacity_for(&self.registry, 1)?;
        }
        let is_bye = request.method == SipMethod::Bye;
        let mut dialog = self
            .dialogs
            .get(&call_id)
            .cloned()
            .ok_or(EngineError::UnknownDialog)?;
        dialog.receive_request(&request)?;
        let local_tag = dialog.local_tag().to_owned();
        let response = response_for_request(&request, 200, "OK", Some(&local_tag), Vec::new())?;
        let mut transaction = ServerTransaction::new(request, reliability, self.config.timers)?;
        transaction.send_final(response.clone(), now)?;
        let terminated = transaction.state() == ServerState::Terminated;

        let mut registry = self.registry.clone();
        if is_bye && dialog.state() == DialogState::Terminated {
            registry.apply(&call_id, CallCommand::Hangup)?;
            registry.apply(&call_id, CallCommand::End)?;
        }
        self.registry = registry;
        self.dialogs.insert(call_id.clone(), dialog);
        if !terminated {
            self.server_transactions.insert(key.clone(), transaction);
            self.server_destinations.insert(key.clone(), source);
            self.server_calls.insert(key, call_id.clone());
        }

        if is_bye {
            self.remove_final_invites_for_call(&call_id);
            self.remove_final_server_invites_for_call(&call_id);
            self.dialogs.remove(&call_id);
        }
        self.finish(vec![SendAction {
            destination: source,
            message: SipMessage::Response(response),
        }])
    }

    fn receive_prack(
        &mut self,
        source: SocketAddr,
        request: SipRequest,
        branch: String,
        now: Duration,
        reliability: TransportReliability,
    ) -> Result<EngineOutput, EngineError> {
        let sip_call_id = required_header(&request.headers, "Call-ID")?.to_owned();
        let call_id = self
            .find_call_by_sip_call_id(&sip_call_id)
            .ok_or(EngineError::UnknownDialog)?;
        let reliable = self
            .reliable_provisionals
            .get(&call_id)
            .cloned()
            .ok_or(EngineError::PrackNotExpected)?;
        let (rseq, invite_sequence) = parse_rack(&request.headers)?;
        if (rseq, invite_sequence) != (reliable.rseq, reliable.invite_sequence) {
            return Err(EngineError::InvalidPrack);
        }
        let key = transaction_key(&branch, &SipMethod::Prack);
        self.ensure_transaction_capacity()?;
        self.ensure_new_transaction(&key)?;
        let mut dialog = self
            .dialogs
            .get(&call_id)
            .cloned()
            .ok_or(EngineError::UnknownDialog)?;
        dialog.receive_request(&request)?;
        let local_tag = dialog.local_tag().to_owned();
        let response = response_for_request(&request, 200, "OK", Some(&local_tag), Vec::new())?;
        let mut transaction = ServerTransaction::new(request, reliability, self.config.timers)?;
        transaction.send_final(response.clone(), now)?;
        let terminated = transaction.state() == ServerState::Terminated;

        self.dialogs.insert(call_id.clone(), dialog);
        self.reliable_provisionals.remove(&call_id);
        if !terminated {
            self.server_transactions.insert(key.clone(), transaction);
            self.server_destinations.insert(key.clone(), source);
            self.server_calls.insert(key, call_id);
        }
        self.finish(vec![SendAction {
            destination: source,
            message: SipMessage::Response(response),
        }])
    }

    fn fail_call(&mut self, id: &CallId) -> Result<(), EngineError> {
        let state = self.registry.snapshot(id)?.state;
        if matches!(state, CallState::Ended | CallState::Failed) {
            return Ok(());
        }
        if state == CallState::Ending {
            self.registry.apply(id, CallCommand::End)?;
            return Ok(());
        }
        self.ensure_event_capacity(1)?;
        self.registry.apply(id, CallCommand::Fail)?;
        if matches!(
            self.registry.snapshot(id)?.state,
            CallState::Failed | CallState::Ending
        ) {
            self.registry.apply(id, CallCommand::End)?;
        }
        self.remove_reliable_provisional_state(id);
        Ok(())
    }

    fn remove_client_transaction(&mut self, branch: &str) {
        self.client_transactions.remove(branch);
        self.client_destinations.remove(branch);
        self.client_calls.remove(branch);
    }

    fn remove_server_transaction(&mut self, branch: &str) {
        self.server_transactions.remove(branch);
        self.server_destinations.remove(branch);
        self.server_calls.remove(branch);
    }

    fn remove_final_invites_for_call(&mut self, id: &CallId) {
        self.final_invites
            .retain(|_, invite| invite.call_id != id.clone());
    }

    fn remove_final_server_invites_for_call(&mut self, id: &CallId) {
        self.final_server_invites
            .retain(|_, invite| invite.call_id != id.clone());
    }

    fn remove_reliable_provisional_state(&mut self, id: &CallId) {
        self.received_reliable_rseq.remove(id);
        self.reliable_provisionals.remove(id);
    }

    fn find_call_by_sip_call_id(&self, sip_call_id: &str) -> Option<CallId> {
        self.dialogs
            .iter()
            .find(|(_, dialog)| dialog.call_id() == sip_call_id)
            .map(|(id, _)| id.clone())
    }

    fn allocate_local_tag(&mut self) -> Result<String, EngineError> {
        let sequence = self.next_local_tag;
        self.next_local_tag = sequence.checked_add(1).ok_or(EngineError::InvalidConfig)?;
        Ok(format!("rust-{sequence}"))
    }

    fn allocate_unique_client_branch(&self, next_branch: &mut u64) -> Result<String, EngineError> {
        loop {
            let sequence = *next_branch;
            *next_branch = sequence.checked_add(1).ok_or(EngineError::InvalidConfig)?;
            let branch = format!("z9hG4bKrust-auth-{sequence}");
            let key = transaction_key(&branch, &SipMethod::Invite);
            if self.ensure_new_transaction(&key).is_ok() {
                return Ok(branch);
            }
        }
    }

    fn ensure_transaction_capacity(&self) -> Result<(), EngineError> {
        if self.transaction_count() >= self.config.max_transactions {
            Err(EngineError::TransactionLimitReached)
        } else {
            Ok(())
        }
    }

    fn ensure_new_transaction(&self, key: &str) -> Result<(), EngineError> {
        if self.client_transactions.contains_key(key)
            || self.server_transactions.contains_key(key)
            || self.final_invites.contains_key(key)
            || self.final_server_invites.contains_key(key)
        {
            Err(EngineError::DuplicateTransaction)
        } else {
            Ok(())
        }
    }

    fn ensure_event_capacity(&self, count: usize) -> Result<(), EngineError> {
        ensure_event_capacity_for(&self.registry, count)
    }

    fn finish(&mut self, actions: Vec<SendAction>) -> Result<EngineOutput, EngineError> {
        let events = self.registry.drain_events(usize::MAX)?;
        Ok(EngineOutput { actions, events })
    }
}

fn required_header<'a>(headers: &'a Headers, name: &'static str) -> Result<&'a str, EngineError> {
    header_value(headers, name).ok_or(EngineError::MissingHeader(name))
}

fn header_value<'a>(headers: &'a Headers, name: &str) -> Option<&'a str> {
    let aliases: &[&str] = match name {
        "Via" => &["Via", "v"],
        "From" => &["From", "f"],
        "To" => &["To", "t"],
        "Call-ID" => &["Call-ID", "i"],
        "CSeq" => &["CSeq", "c"],
        _ => &[name],
    };
    headers
        .iter()
        .find(|header| {
            aliases
                .iter()
                .any(|alias| header.name.eq_ignore_ascii_case(alias))
        })
        .map(|header| header.value.as_str())
}

fn ensure_event_capacity_for(registry: &CallRegistry, count: usize) -> Result<(), EngineError> {
    let pending = registry.pending_events();
    let required = pending.checked_add(count).ok_or(ApiError::EventQueueFull)?;
    if required > registry.config().max_pending_events {
        Err(ApiError::EventQueueFull.into())
    } else {
        Ok(())
    }
}

fn transaction_branch(headers: &Headers) -> Result<String, EngineError> {
    let via = required_header(headers, "Via")?;
    let first = via.split(',').next().unwrap_or_default();
    let branch = first
        .split(';')
        .skip(1)
        .find_map(|parameter| {
            let (name, value) = parameter.trim().split_once('=')?;
            name.eq_ignore_ascii_case("branch").then_some(value.trim())
        })
        .filter(|value| {
            !value.is_empty()
                && value.len() <= DEFAULT_MAX_BRANCH_BYTES
                && value.bytes().all(|byte| byte.is_ascii_graphic())
        })
        .ok_or(EngineError::InvalidBranch)?;
    Ok(branch.to_owned())
}

fn cseq_method(headers: &Headers) -> Result<SipMethod, EngineError> {
    let value = required_header(headers, "CSeq")?;
    value
        .split_whitespace()
        .nth(1)
        .and_then(SipMethod::parse)
        .ok_or(EngineError::InvalidCSeq)
}

fn validate_request_cseq(headers: &Headers, method: &SipMethod) -> Result<(), EngineError> {
    let value = required_header(headers, "CSeq")?;
    let mut fields = value.split_whitespace();
    let _sequence = fields
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or(EngineError::InvalidCSeq)?;
    let cseq_method = fields
        .next()
        .and_then(SipMethod::parse)
        .ok_or(EngineError::InvalidCSeq)?;
    if fields.next().is_some() || cseq_method != *method {
        return Err(EngineError::InvalidCSeq);
    }
    Ok(())
}

fn digest_header_names(status_code: u16) -> Result<(&'static str, &'static str), EngineError> {
    match status_code {
        401 => Ok(("WWW-Authenticate", "Authorization")),
        407 => Ok(("Proxy-Authenticate", "Proxy-Authorization")),
        _ => Err(EngineError::InvalidResponseStatus),
    }
}

fn invite_response_key(response: &SipResponse) -> Result<String, EngineError> {
    let branch = transaction_branch(&response.headers)?;
    let method = cseq_method(&response.headers)?;
    if method != SipMethod::Invite {
        return Err(EngineError::UnsupportedRequest);
    }
    Ok(transaction_key(&branch, &method))
}

fn build_digest_retry_request(
    request: &SipRequest,
    branch: &str,
    authorization_header: &'static str,
    challenge: &DigestChallenge,
    credentials: &DigestCredentials,
    cnonce: Option<&str>,
    nonce_count: Option<u32>,
) -> Result<SipRequest, EngineError> {
    let authorization = DigestAuthorization::from_credentials(
        credentials,
        challenge,
        request.method.as_str(),
        &request.request_uri,
        &request.body,
        cnonce,
        nonce_count,
    )?;
    build_authenticated_invite(
        request,
        branch,
        authorization_header,
        &authorization.to_header_value(),
    )
}

fn build_authenticated_invite(
    request: &SipRequest,
    branch: &str,
    authorization_header: &'static str,
    authorization_value: &str,
) -> Result<SipRequest, EngineError> {
    let next_cseq = invite_cseq(&request.headers)?
        .checked_add(1)
        .ok_or(EngineError::InvalidCSeq)?;
    let mut headers = Headers::new();
    let mut replaced_via = false;
    let mut replaced_cseq = false;

    for header in request.headers.iter() {
        if header.name.eq_ignore_ascii_case("Authorization")
            || header.name.eq_ignore_ascii_case("Proxy-Authorization")
        {
            continue;
        }
        if header.name.eq_ignore_ascii_case("Via") || header.name.eq_ignore_ascii_case("v") {
            if replaced_via {
                headers.push(header.name.clone(), header.value.clone());
            } else {
                headers.push(
                    header.name.clone(),
                    replace_top_via_branch(&header.value, branch)?,
                );
                replaced_via = true;
            }
            continue;
        }
        if header.name.eq_ignore_ascii_case("CSeq") || header.name.eq_ignore_ascii_case("c") {
            if !replaced_cseq {
                headers.push(header.name.clone(), format!("{next_cseq} INVITE"));
                replaced_cseq = true;
            }
            continue;
        }
        headers.push(header.name.clone(), header.value.clone());
    }

    if !replaced_via {
        return Err(EngineError::MissingHeader("Via"));
    }
    if !replaced_cseq {
        return Err(EngineError::MissingHeader("CSeq"));
    }
    headers.push(authorization_header, authorization_value);
    Ok(SipRequest {
        method: SipMethod::Invite,
        request_uri: request.request_uri.clone(),
        version: request.version.clone(),
        headers,
        body: request.body.clone(),
    })
}

fn invite_cseq(headers: &Headers) -> Result<u32, EngineError> {
    let value = required_header(headers, "CSeq")?;
    let mut fields = value.split_whitespace();
    let sequence = fields
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or(EngineError::InvalidCSeq)?;
    let method = fields
        .next()
        .and_then(SipMethod::parse)
        .ok_or(EngineError::InvalidCSeq)?;
    if method != SipMethod::Invite || fields.next().is_some() {
        return Err(EngineError::InvalidCSeq);
    }
    Ok(sequence)
}

fn replace_top_via_branch(value: &str, branch: &str) -> Result<String, EngineError> {
    let (top_via, remainder) = value
        .split_once(',')
        .map_or((value, None), |(top, rest)| (top, Some(rest)));
    let mut parts = top_via.split(';');
    let sent_by = parts.next().unwrap_or_default().trim();
    if sent_by.is_empty() {
        return Err(EngineError::InvalidBranch);
    }
    let mut rewritten = sent_by.to_owned();
    for parameter in parts {
        let parameter = parameter.trim();
        if parameter.is_empty()
            || parameter
                .split_once('=')
                .is_some_and(|(name, _)| name.trim().eq_ignore_ascii_case("branch"))
        {
            continue;
        }
        rewritten.push(';');
        rewritten.push_str(parameter);
    }
    rewritten.push_str(";branch=");
    rewritten.push_str(branch);
    if let Some(remainder) = remainder {
        rewritten.push(',');
        rewritten.push_str(remainder);
    }
    Ok(rewritten)
}

fn transaction_key(branch: &str, method: &SipMethod) -> String {
    format!("{branch}:{}", method.as_str())
}

fn request_has_to_tag(request: &SipRequest) -> bool {
    header_value(&request.headers, "To").is_some_and(value_has_tag)
}

fn value_has_tag(value: &str) -> bool {
    value.split(';').skip(1).any(|parameter| {
        parameter
            .trim()
            .split_once('=')
            .is_some_and(|(name, tag)| name.eq_ignore_ascii_case("tag") && !tag.trim().is_empty())
    })
}

fn response_for_request(
    request: &SipRequest,
    status_code: u16,
    reason: &str,
    local_tag: Option<&str>,
    body: Vec<u8>,
) -> Result<SipResponse, EngineError> {
    let mut headers = Headers::new();
    for header in request.headers.iter().filter(|header| {
        header.name.eq_ignore_ascii_case("Via") || header.name.eq_ignore_ascii_case("v")
    }) {
        headers.push("Via", header.value.clone());
    }
    headers.push("From", required_header(&request.headers, "From")?);
    let mut to = required_header(&request.headers, "To")?.to_owned();
    if let Some(local_tag) = local_tag {
        if !value_has_tag(&to) {
            to.push_str(";tag=");
            to.push_str(local_tag);
        }
    }
    headers.push("To", to);
    headers.push("Call-ID", required_header(&request.headers, "Call-ID")?);
    headers.push("CSeq", required_header(&request.headers, "CSeq")?);
    Ok(SipResponse {
        version: "SIP/2.0".to_owned(),
        status_code,
        reason: reason.to_owned(),
        headers,
        body,
    })
}

fn response_call_command(state: CallState, status_code: u16) -> Option<CallCommand> {
    match status_code {
        183 if state == CallState::Inviting => Some(CallCommand::EarlyMedia),
        180 if matches!(state, CallState::Inviting | CallState::Early) => {
            Some(CallCommand::Ringing)
        }
        200..=299
            if matches!(
                state,
                CallState::Inviting | CallState::Early | CallState::Ringing
            ) =>
        {
            Some(CallCommand::Answer)
        }
        300..=699
            if !matches!(
                state,
                CallState::Ended | CallState::Failed | CallState::Ending
            ) =>
        {
            Some(CallCommand::Fail)
        }
        _ => None,
    }
}

fn build_ack(
    next_branch: &mut u64,
    request: &SipRequest,
    response: &SipResponse,
    dialog: &Dialog,
    successful: bool,
) -> Result<SipRequest, EngineError> {
    let sequence = header_value(&request.headers, "CSeq")
        .and_then(|value| value.split_whitespace().next())
        .ok_or(EngineError::MissingHeader("CSeq"))?;
    let request_via = required_header(&request.headers, "Via")?;
    let first_via = request_via.split(',').next().unwrap_or_default().trim();
    if first_via.is_empty() {
        return Err(EngineError::InvalidBranch);
    }
    let response_to = required_header(&response.headers, "To")?;
    let from = required_header(&request.headers, "From")?;
    let call_id = required_header(&request.headers, "Call-ID")?;
    let mut headers = Headers::new();
    let via = if successful {
        let sequence = *next_branch;
        *next_branch = sequence.checked_add(1).ok_or(EngineError::InvalidConfig)?;
        let via_prefix = first_via.split(';').next().unwrap_or(first_via).trim();
        format!("{via_prefix};branch=z9hG4bKrust-{sequence}")
    } else {
        first_via.to_owned()
    };
    headers.push("Via", via);
    headers.push("Max-Forwards", "70");
    headers.push("From", from);
    headers.push("To", response_to);
    headers.push("Call-ID", call_id);
    headers.push("CSeq", format!("{sequence} ACK"));
    for route in dialog.route_set() {
        headers.push("Route", format!("<{route}>"));
    }
    Ok(SipRequest {
        method: SipMethod::Ack,
        request_uri: if successful {
            dialog.remote_target().to_owned()
        } else {
            request.request_uri.clone()
        },
        version: "SIP/2.0".to_owned(),
        headers,
        body: Vec::new(),
    })
}

fn build_prack(
    next_branch: &mut u64,
    invite: &SipRequest,
    response: &SipResponse,
    dialog: &mut Dialog,
    rseq: u32,
    invite_sequence: u32,
) -> Result<SipRequest, EngineError> {
    let cseq = dialog.next_local_sequence_for(SipMethod::Prack)?;
    let branch_sequence = *next_branch;
    *next_branch = branch_sequence
        .checked_add(1)
        .ok_or(EngineError::InvalidConfig)?;
    let via = replace_top_via_branch(
        required_header(&invite.headers, "Via")?,
        &format!("z9hG4bKrust-prack-{branch_sequence}"),
    )?;
    let mut headers = Headers::new();
    headers.push("Via", via);
    headers.push("Max-Forwards", "70");
    headers.push("From", required_header(&invite.headers, "From")?);
    headers.push("To", required_header(&response.headers, "To")?);
    headers.push("Call-ID", required_header(&invite.headers, "Call-ID")?);
    headers.push("CSeq", format!("{cseq} PRACK"));
    headers.push("RAck", format!("{rseq} {invite_sequence} INVITE"));
    for route in dialog.route_set() {
        headers.push("Route", format!("<{route}>"));
    }
    Ok(SipRequest {
        method: SipMethod::Prack,
        request_uri: dialog.remote_target().to_owned(),
        version: "SIP/2.0".to_owned(),
        headers,
        body: Vec::new(),
    })
}

fn reliable_provisional_rseq(response: &SipResponse) -> Result<Option<u32>, EngineError> {
    if !(101..200).contains(&response.status_code) {
        return Ok(None);
    }
    let require_100rel = header_has_token(&response.headers, "Require", "100rel");
    let Some(value) = header_value(&response.headers, "RSeq") else {
        return if require_100rel {
            Err(EngineError::InvalidPrack)
        } else {
            Ok(None)
        };
    };
    let rseq = value
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|rseq| *rseq > 0)
        .ok_or(EngineError::InvalidPrack)?;
    Ok(Some(rseq))
}

fn parse_rack(headers: &Headers) -> Result<(u32, u32), EngineError> {
    let value = required_header(headers, "RAck")?;
    let mut fields = value.split_whitespace();
    let rseq = fields
        .next()
        .and_then(|field| field.parse::<u32>().ok())
        .filter(|rseq| *rseq > 0)
        .ok_or(EngineError::InvalidPrack)?;
    let invite_sequence = fields
        .next()
        .and_then(|field| field.parse::<u32>().ok())
        .ok_or(EngineError::InvalidPrack)?;
    let method = fields
        .next()
        .and_then(SipMethod::parse)
        .ok_or(EngineError::InvalidPrack)?;
    if fields.next().is_some() || method != SipMethod::Invite {
        return Err(EngineError::InvalidPrack);
    }
    Ok((rseq, invite_sequence))
}

fn header_has_token(headers: &Headers, name: &str, expected: &str) -> bool {
    headers.get_all(name).any(|value| {
        value
            .split(',')
            .any(|token| token.trim().eq_ignore_ascii_case(expected))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use call_core::CallEventKind;
    use std::net::{IpAddr, Ipv4Addr};

    fn address(port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
    }

    fn invite(branch: &str, call_id: &str, to: &str, cseq: &str) -> SipRequest {
        let mut headers = Headers::new();
        headers.push("Via", format!("SIP/2.0/UDP peer.invalid;branch={branch}"));
        headers.push("From", "Alice <sip:alice@example.com>;tag=alice-1");
        headers.push("To", to);
        headers.push("Call-ID", call_id);
        headers.push("CSeq", cseq);
        headers.push("Contact", "<sip:alice@127.0.0.1:5060>");
        SipRequest {
            method: SipMethod::Invite,
            request_uri: "sip:bob@example.com".to_owned(),
            version: "SIP/2.0".to_owned(),
            headers,
            body: Vec::new(),
        }
    }

    fn ack_for(request: &SipRequest, response: &SipResponse, branch: &str) -> SipRequest {
        let mut headers = Headers::new();
        headers.push("Via", format!("SIP/2.0/UDP peer.invalid;branch={branch}"));
        headers.push("From", request.headers.get("From").unwrap());
        headers.push("To", response.headers.get("To").unwrap());
        headers.push("Call-ID", request.headers.get("Call-ID").unwrap());
        headers.push("CSeq", "1 ACK");
        SipRequest {
            method: SipMethod::Ack,
            request_uri: request.request_uri.clone(),
            version: "SIP/2.0".to_owned(),
            headers,
            body: Vec::new(),
        }
    }

    fn options(branch: &str, call_id: &str) -> SipRequest {
        let mut headers = Headers::new();
        headers.push("Via", format!("SIP/2.0/UDP peer.invalid;branch={branch}"));
        headers.push("From", "Alice <sip:alice@example.com>;tag=alice-1");
        headers.push("To", "Bob <sip:bob@example.com>");
        headers.push("Call-ID", call_id);
        headers.push("CSeq", "1 OPTIONS");
        SipRequest {
            method: SipMethod::Options,
            request_uri: "sip:bob@example.com".to_owned(),
            version: "SIP/2.0".to_owned(),
            headers,
            body: Vec::new(),
        }
    }

    fn digest_challenge(
        request: &SipRequest,
        status_code: u16,
        header_name: &str,
        header_value: &str,
    ) -> SipResponse {
        let mut headers = Headers::new();
        headers.push("Via", request.headers.get("Via").unwrap());
        headers.push("From", request.headers.get("From").unwrap());
        headers.push("To", "Bob <sip:bob@example.com>;tag=digest-peer");
        headers.push("Call-ID", request.headers.get("Call-ID").unwrap());
        headers.push("CSeq", request.headers.get("CSeq").unwrap());
        headers.push(header_name, header_value);
        SipResponse {
            version: "SIP/2.0".to_owned(),
            status_code,
            reason: if status_code == 401 {
                "Unauthorized"
            } else {
                "Proxy Authentication Required"
            }
            .to_owned(),
            headers,
            body: Vec::new(),
        }
    }

    fn successful_response(request: &SipRequest) -> SipResponse {
        successful_response_with_tag(request, "authenticated-peer")
    }

    fn successful_response_with_tag(request: &SipRequest, tag: &str) -> SipResponse {
        let mut headers = Headers::new();
        headers.push("Via", request.headers.get("Via").unwrap());
        headers.push("From", request.headers.get("From").unwrap());
        headers.push("To", format!("Bob <sip:bob@example.com>;tag={tag}"));
        headers.push("Call-ID", request.headers.get("Call-ID").unwrap());
        headers.push("CSeq", request.headers.get("CSeq").unwrap());
        headers.push("Contact", "<sip:bob@127.0.0.1:5070>");
        SipResponse {
            version: "SIP/2.0".to_owned(),
            status_code: 200,
            reason: "OK".to_owned(),
            headers,
            body: Vec::new(),
        }
    }

    fn reliable_provisional_response(request: &SipRequest, rseq: u32) -> SipResponse {
        let mut headers = Headers::new();
        headers.push("Via", request.headers.get("Via").unwrap());
        headers.push("From", request.headers.get("From").unwrap());
        headers.push("To", "Bob <sip:bob@example.com>;tag=provisional-peer");
        headers.push("Call-ID", request.headers.get("Call-ID").unwrap());
        headers.push("CSeq", request.headers.get("CSeq").unwrap());
        headers.push("Require", "100rel");
        headers.push("RSeq", rseq.to_string());
        SipResponse {
            version: "SIP/2.0".to_owned(),
            status_code: 183,
            reason: "Session Progress".to_owned(),
            headers,
            body: Vec::new(),
        }
    }

    fn reliable_provisional_without_rseq(request: &SipRequest) -> SipResponse {
        let mut response = reliable_provisional_response(request, 1);
        response.headers = {
            let mut headers = Headers::new();
            headers.push("Via", request.headers.get("Via").unwrap());
            headers.push("From", request.headers.get("From").unwrap());
            headers.push("To", "Bob <sip:bob@example.com>;tag=provisional-peer");
            headers.push("Call-ID", request.headers.get("Call-ID").unwrap());
            headers.push("CSeq", request.headers.get("CSeq").unwrap());
            headers.push("Require", "100rel");
            headers
        };
        response
    }

    fn prack_response(request: &SipRequest) -> SipResponse {
        let mut headers = Headers::new();
        headers.push("Via", request.headers.get("Via").unwrap());
        headers.push("From", request.headers.get("From").unwrap());
        headers.push("To", "Bob <sip:bob@example.com>;tag=provisional-peer");
        headers.push("Call-ID", request.headers.get("Call-ID").unwrap());
        headers.push("CSeq", request.headers.get("CSeq").unwrap());
        SipResponse {
            version: "SIP/2.0".to_owned(),
            status_code: 200,
            reason: "OK".to_owned(),
            headers,
            body: Vec::new(),
        }
    }

    fn inbound_prack(
        invite: &SipRequest,
        provisional: &SipResponse,
        branch: &str,
        cseq: u32,
        rack: &str,
    ) -> SipRequest {
        let mut headers = Headers::new();
        headers.push("Via", format!("SIP/2.0/UDP peer.invalid;branch={branch}"));
        headers.push("From", invite.headers.get("From").unwrap());
        headers.push("To", provisional.headers.get("To").unwrap());
        headers.push("Call-ID", invite.headers.get("Call-ID").unwrap());
        headers.push("CSeq", format!("{cseq} PRACK"));
        headers.push("RAck", rack);
        SipRequest {
            method: SipMethod::Prack,
            request_uri: invite.request_uri.clone(),
            version: invite.version.clone(),
            headers,
            body: Vec::new(),
        }
    }

    fn complete_and_reclaim_authenticated_call(
        engine: &mut CallEngine,
        call_id: &CallId,
        retry: &SipRequest,
    ) {
        let completed = engine
            .receive_response(successful_response(retry), Duration::from_millis(20))
            .unwrap();
        assert!(matches!(
            completed.actions()[0].message,
            SipMessage::Request(SipRequest {
                method: SipMethod::Ack,
                ..
            })
        ));
        assert_eq!(engine.snapshot(call_id).unwrap().state, CallState::Answered);
        engine
            .apply_call_command(call_id, CallCommand::Hangup)
            .unwrap();
        engine
            .apply_call_command(call_id, CallCommand::End)
            .unwrap();
        engine.reclaim_terminal_call(call_id).unwrap();
        assert!(!engine.digest_retries.contains_key(call_id));
    }

    #[test]
    fn inbound_invite_is_bounded_answered_and_acknowledged() {
        let mut engine = CallEngine::new(EngineConfig::default()).unwrap();
        let request = invite(
            "in-1",
            "sip-call-1",
            "Bob <sip:bob@example.com>",
            "1 INVITE",
        );
        let source = address(5060);
        let output = engine
            .receive_request(
                source,
                request.clone(),
                Duration::ZERO,
                TransportReliability::Unreliable,
            )
            .unwrap();
        assert_eq!(output.actions().len(), 1);
        assert_eq!(output.events()[0].kind, CallEventKind::Created);
        assert_eq!(output.events()[1].kind, CallEventKind::InviteReceived);
        let call_id = output.events()[0].call_id.clone();
        assert_eq!(
            engine.snapshot(&call_id).unwrap().state,
            CallState::Inviting
        );

        let output = engine
            .respond_to_invite(&call_id, 200, "OK", Vec::new(), Duration::ZERO)
            .unwrap();
        let SipMessage::Response(response) = &output.actions()[0].message else {
            panic!("expected response");
        };
        assert_eq!(response.status_code, 200);
        assert!(
            output
                .events()
                .iter()
                .any(|event| event.kind == CallEventKind::Answered)
        );
        assert_eq!(
            engine.snapshot(&call_id).unwrap().state,
            CallState::Answered
        );

        let retransmitted = engine
            .receive_request(
                source,
                request.clone(),
                Duration::from_millis(1),
                TransportReliability::Unreliable,
            )
            .unwrap();
        assert!(matches!(
            retransmitted.actions()[0].message,
            SipMessage::Response(SipResponse {
                status_code: 200,
                ..
            })
        ));
        let polled = engine.poll(Duration::from_millis(500)).unwrap();
        assert!(polled.actions().iter().any(|action| matches!(
            action.message,
            SipMessage::Response(SipResponse {
                status_code: 200,
                ..
            })
        )));

        engine
            .receive_request(
                source,
                ack_for(&request, response, "ack-1"),
                Duration::ZERO,
                TransportReliability::Unreliable,
            )
            .unwrap();
        assert!(
            engine
                .poll(Duration::from_secs(2))
                .unwrap()
                .actions()
                .is_empty()
        );
        assert_eq!(engine.transaction_count(), 0);
    }

    #[test]
    fn outbound_invite_learns_dialog_and_emits_ack_on_success() {
        let mut engine = CallEngine::new(EngineConfig::default()).unwrap();
        let request = invite(
            "out-1",
            "sip-call-2",
            "Bob <sip:bob@example.com>",
            "7 INVITE",
        );
        let destination = address(5061);
        let (call_id, created) = engine
            .originate(
                request.clone(),
                destination,
                Duration::ZERO,
                TransportReliability::Unreliable,
            )
            .unwrap();
        assert_eq!(created.events().len(), 2);

        let mut headers = Headers::new();
        headers.push("Via", "SIP/2.0/UDP peer.invalid;branch=out-1");
        headers.push("From", request.headers.get("From").unwrap());
        headers.push("To", "Bob <sip:bob@example.com>;tag=remote-1");
        headers.push("Call-ID", "sip-call-2");
        headers.push("CSeq", "7 INVITE");
        headers.push("Contact", "<sip:bob@127.0.0.1:5070>");
        let response = SipResponse {
            version: "SIP/2.0".to_owned(),
            status_code: 200,
            reason: "OK".to_owned(),
            headers,
            body: Vec::new(),
        };
        let output = engine.receive_response(response, Duration::ZERO).unwrap();
        assert_eq!(output.actions().len(), 1);
        assert!(matches!(
            output.actions()[0].message,
            SipMessage::Request(SipRequest {
                method: SipMethod::Ack,
                ..
            })
        ));
        assert_eq!(
            engine.snapshot(&call_id).unwrap().state,
            CallState::Answered
        );
    }

    #[test]
    fn outbound_reliable_provisional_emits_prack_and_duplicate_replays_nothing() {
        let mut engine = CallEngine::new(EngineConfig::default()).unwrap();
        let request = invite(
            "prack-out-1",
            "sip-call-prack-out",
            "Bob <sip:bob@example.com>",
            "7 INVITE",
        );
        let (call_id, _) = engine
            .originate(
                request.clone(),
                address(5061),
                Duration::ZERO,
                TransportReliability::Unreliable,
            )
            .unwrap();
        let provisional = reliable_provisional_response(&request, 1);
        let first = engine
            .receive_response(provisional.clone(), Duration::from_millis(1))
            .unwrap();
        assert_eq!(first.actions().len(), 1);
        let SipMessage::Request(prack) = &first.actions()[0].message else {
            panic!("expected PRACK");
        };
        assert_eq!(prack.method, SipMethod::Prack);
        assert_eq!(prack.headers.get("CSeq"), Some("8 PRACK"));
        assert_eq!(prack.headers.get("RAck"), Some("1 7 INVITE"));
        assert_eq!(prack.headers.get("To"), provisional.headers.get("To"));
        assert_eq!(engine.snapshot(&call_id).unwrap().state, CallState::Early);
        assert_eq!(engine.transaction_count(), 2);

        let duplicate = engine
            .receive_response(provisional, Duration::from_millis(2))
            .unwrap();
        assert!(duplicate.actions().is_empty());
        assert_eq!(engine.transaction_count(), 2);

        let acknowledged = engine
            .receive_response(prack_response(prack), Duration::from_millis(3))
            .unwrap();
        assert!(acknowledged.actions().is_empty());
        assert!(acknowledged.events().is_empty());
        assert_eq!(engine.snapshot(&call_id).unwrap().state, CallState::Early);

        let late_duplicate = engine
            .receive_response(
                reliable_provisional_response(&request, 1),
                Duration::from_millis(4),
            )
            .unwrap();
        assert!(late_duplicate.actions().is_empty());

        let completed = engine
            .receive_response(
                successful_response_with_tag(&request, "provisional-peer"),
                Duration::from_millis(5),
            )
            .unwrap();
        assert!(matches!(
            completed.actions()[0].message,
            SipMessage::Request(SipRequest {
                method: SipMethod::Ack,
                ..
            })
        ));
        assert_eq!(
            engine.snapshot(&call_id).unwrap().state,
            CallState::Answered
        );
        assert!(!engine.received_reliable_rseq.contains_key(&call_id));
    }

    #[test]
    fn inbound_reliable_provisional_requires_matching_prack_and_replays_response() {
        let mut engine = CallEngine::new(EngineConfig::default()).unwrap();
        let mut request = invite(
            "prack-in-1",
            "sip-call-prack-in",
            "Bob <sip:bob@example.com>",
            "42 INVITE",
        );
        request.headers.push("Require", "100rel");
        let source = address(5060);
        let created = engine
            .receive_request(
                source,
                request.clone(),
                Duration::ZERO,
                TransportReliability::Unreliable,
            )
            .unwrap();
        let call_id = created.events()[0].call_id.clone();
        let provisional = engine
            .respond_to_invite(
                &call_id,
                183,
                "Session Progress",
                Vec::new(),
                Duration::from_millis(1),
            )
            .unwrap();
        let SipMessage::Response(provisional_response) = &provisional.actions()[0].message else {
            panic!("expected reliable provisional response");
        };
        assert_eq!(provisional_response.headers.get("Require"), Some("100rel"));
        assert_eq!(provisional_response.headers.get("RSeq"), Some("1"));

        let transactions = engine.transaction_count();
        let mismatched = inbound_prack(
            &request,
            provisional_response,
            "prack-in-mismatch",
            43,
            "9 42 INVITE",
        );
        assert_eq!(
            engine
                .receive_request(
                    source,
                    mismatched,
                    Duration::from_millis(2),
                    TransportReliability::Unreliable,
                )
                .unwrap_err(),
            EngineError::InvalidPrack
        );
        assert_eq!(engine.transaction_count(), transactions);
        assert_eq!(engine.snapshot(&call_id).unwrap().state, CallState::Early);

        let prack = inbound_prack(
            &request,
            provisional_response,
            "prack-in-2",
            43,
            "1 42 INVITE",
        );
        let acknowledged = engine
            .receive_request(
                source,
                prack.clone(),
                Duration::from_millis(3),
                TransportReliability::Unreliable,
            )
            .unwrap();
        assert!(matches!(
            acknowledged.actions()[0].message,
            SipMessage::Response(SipResponse {
                status_code: 200,
                ..
            })
        ));
        assert_eq!(engine.snapshot(&call_id).unwrap().state, CallState::Early);

        let duplicate = engine
            .receive_request(
                source,
                prack,
                Duration::from_millis(4),
                TransportReliability::Unreliable,
            )
            .unwrap();
        assert!(matches!(
            duplicate.actions()[0].message,
            SipMessage::Response(SipResponse {
                status_code: 200,
                ..
            })
        ));

        engine
            .respond_to_invite(&call_id, 200, "OK", Vec::new(), Duration::from_millis(5))
            .unwrap();
        assert!(!engine.reliable_provisionals.contains_key(&call_id));
    }

    #[test]
    fn malformed_reliable_provisional_and_rack_are_atomic() {
        let mut engine = CallEngine::new(EngineConfig::default()).unwrap();
        let request = invite(
            "prack-atomic",
            "sip-call-prack-atomic",
            "Bob <sip:bob@example.com>",
            "7 INVITE",
        );
        let (call_id, _) = engine
            .originate(
                request.clone(),
                address(5061),
                Duration::ZERO,
                TransportReliability::Unreliable,
            )
            .unwrap();
        let mut malformed = reliable_provisional_response(&request, 1);
        malformed.headers = {
            let mut headers = Headers::new();
            headers.push("Via", request.headers.get("Via").unwrap());
            headers.push("From", request.headers.get("From").unwrap());
            headers.push("To", "Bob <sip:bob@example.com>;tag=provisional-peer");
            headers.push("Call-ID", request.headers.get("Call-ID").unwrap());
            headers.push("CSeq", request.headers.get("CSeq").unwrap());
            headers.push("Require", "100rel");
            headers.push("RSeq", "not-a-number");
            headers
        };
        let transactions = engine.transaction_count();
        assert_eq!(
            engine
                .receive_response(malformed, Duration::from_millis(1))
                .unwrap_err(),
            EngineError::InvalidPrack
        );
        assert_eq!(engine.transaction_count(), transactions);
        assert_eq!(
            engine.snapshot(&call_id).unwrap().state,
            CallState::Inviting
        );

        assert_eq!(
            engine
                .receive_response(
                    reliable_provisional_without_rseq(&request),
                    Duration::from_millis(2),
                )
                .unwrap_err(),
            EngineError::InvalidPrack
        );
        assert_eq!(engine.transaction_count(), transactions);
        assert_eq!(
            engine.snapshot(&call_id).unwrap().state,
            CallState::Inviting
        );
    }

    #[test]
    fn www_digest_challenge_retries_invite_and_duplicate_only_replays_ack() {
        let mut engine = CallEngine::new(EngineConfig::default()).unwrap();
        let mut request = invite(
            "digest-www",
            "sip-call-digest-www",
            "Bob <sip:bob@example.com>",
            "7 INVITE",
        );
        request.headers.push("Authorization", "Digest stale-value");
        request.body = b"v=0\r\n".to_vec();
        let destination = address(5061);
        let (call_id, _) = engine
            .originate(
                request.clone(),
                destination,
                Duration::ZERO,
                TransportReliability::Unreliable,
            )
            .unwrap();
        let challenge_value =
            r#"Digest realm="carrier", nonce="nonce-1", qop="auth", opaque="opaque-1""#;
        let challenge = digest_challenge(&request, 401, "WWW-Authenticate", challenge_value);
        let credentials = DigestCredentials::new("alice", "never-log-this-password");

        let output = engine
            .receive_digest_challenge(
                challenge.clone(),
                Duration::from_millis(10),
                &credentials,
                Some("client-nonce"),
                Some(1),
            )
            .unwrap();

        assert_eq!(output.actions().len(), 2);
        assert!(output.events().is_empty());
        let SipMessage::Request(ack) = &output.actions()[0].message else {
            panic!("expected challenge ACK");
        };
        assert_eq!(ack.method, SipMethod::Ack);
        assert_eq!(ack.headers.get("Via"), request.headers.get("Via"));
        let SipMessage::Request(retry) = &output.actions()[1].message else {
            panic!("expected authenticated INVITE");
        };
        assert_eq!(retry.method, SipMethod::Invite);
        assert_eq!(retry.request_uri, request.request_uri);
        assert_eq!(retry.body, request.body);
        assert_eq!(retry.headers.get("From"), request.headers.get("From"));
        assert_eq!(retry.headers.get("Call-ID"), request.headers.get("Call-ID"));
        assert_eq!(retry.headers.get("CSeq"), Some("8 INVITE"));
        assert_ne!(retry.headers.get("Via"), request.headers.get("Via"));
        assert!(retry.headers.get("Proxy-Authorization").is_none());
        let authorization =
            DigestAuthorization::parse(retry.headers.get("Authorization").unwrap()).unwrap();
        let parsed_challenge = DigestChallenge::parse(challenge_value).unwrap();
        assert!(authorization.verify_against(
            &parsed_challenge,
            &credentials,
            "INVITE",
            &retry.body,
        ));
        assert_eq!(
            engine.snapshot(&call_id).unwrap().state,
            CallState::Inviting
        );
        assert_eq!(engine.transaction_count(), 2);
        assert!(!format!("{engine:?}").contains("never-log-this-password"));

        let duplicate = engine
            .receive_digest_challenge(
                challenge,
                Duration::from_millis(11),
                &DigestCredentials::new("unused", "also-not-retained"),
                None,
                None,
            )
            .unwrap();
        assert_eq!(duplicate.actions().len(), 1);
        assert!(matches!(
            duplicate.actions()[0].message,
            SipMessage::Request(SipRequest {
                method: SipMethod::Ack,
                ..
            })
        ));
        assert_eq!(engine.transaction_count(), 2);

        complete_and_reclaim_authenticated_call(&mut engine, &call_id, retry);
    }

    #[test]
    fn proxy_digest_without_qop_is_bounded_across_new_transactions() {
        let config = EngineConfig {
            max_digest_retries_per_call: 2,
            ..EngineConfig::default()
        };
        let mut engine = CallEngine::new(config).unwrap();
        let request = invite(
            "digest-proxy",
            "sip-call-digest-proxy",
            "Bob <sip:bob@example.com>",
            "20 INVITE",
        );
        let (call_id, _) = engine
            .originate(
                request.clone(),
                address(5061),
                Duration::ZERO,
                TransportReliability::Unreliable,
            )
            .unwrap();
        let credentials = DigestCredentials::new("proxy-user", "proxy-password");
        let first_challenge_value = r#"Digest realm="proxy", nonce="proxy-nonce-1""#;
        let first_challenge =
            digest_challenge(&request, 407, "Proxy-Authenticate", first_challenge_value);
        let first = engine
            .receive_digest_challenge(
                first_challenge,
                Duration::from_millis(1),
                &credentials,
                None,
                None,
            )
            .unwrap();
        let SipMessage::Request(first_retry) = &first.actions()[1].message else {
            panic!("expected first proxy retry");
        };
        assert_eq!(first_retry.headers.get("CSeq"), Some("21 INVITE"));
        assert!(first_retry.headers.get("Authorization").is_none());
        let first_authorization =
            DigestAuthorization::parse(first_retry.headers.get("Proxy-Authorization").unwrap())
                .unwrap();
        assert!(first_authorization.verify_against(
            &DigestChallenge::parse(first_challenge_value).unwrap(),
            &credentials,
            "INVITE",
            &first_retry.body,
        ));

        let second_challenge = digest_challenge(
            first_retry,
            407,
            "Proxy-Authenticate",
            r#"Digest realm="proxy", nonce="proxy-nonce-2""#,
        );
        let second = engine
            .receive_digest_challenge(
                second_challenge,
                Duration::from_millis(2),
                &credentials,
                None,
                None,
            )
            .unwrap();
        let SipMessage::Request(second_retry) = &second.actions()[1].message else {
            panic!("expected second proxy retry");
        };
        assert_eq!(second_retry.headers.get("CSeq"), Some("22 INVITE"));
        assert_eq!(engine.digest_retries.get(&call_id), Some(&2));

        let third_challenge = digest_challenge(
            second_retry,
            407,
            "Proxy-Authenticate",
            r#"Digest realm="proxy", nonce="proxy-nonce-3""#,
        );
        let before_transactions = engine.transaction_count();
        assert_eq!(
            engine
                .receive_digest_challenge(
                    third_challenge,
                    Duration::from_millis(3),
                    &credentials,
                    None,
                    None,
                )
                .unwrap_err(),
            EngineError::DigestRetryLimitReached
        );
        assert_eq!(engine.transaction_count(), before_transactions);
        assert_eq!(engine.digest_retries.get(&call_id), Some(&2));
        assert_eq!(
            engine.snapshot(&call_id).unwrap().state,
            CallState::Inviting
        );
        assert!(!format!("{engine:?}").contains("proxy-password"));
    }

    #[test]
    fn malformed_digest_retry_is_atomic_and_does_not_consume_retry_budget() {
        let mut engine = CallEngine::new(EngineConfig::default()).unwrap();
        let request = invite(
            "digest-atomic",
            "sip-call-digest-atomic",
            "Bob <sip:bob@example.com>",
            "3 INVITE",
        );
        let (call_id, _) = engine
            .originate(
                request.clone(),
                address(5061),
                Duration::ZERO,
                TransportReliability::Unreliable,
            )
            .unwrap();
        let malformed = digest_challenge(
            &request,
            401,
            "WWW-Authenticate",
            "Digest realm=missing-nonce",
        );
        let credentials = DigestCredentials::new("alice", "atomic-password");
        let transaction_count = engine.transaction_count();
        let error = engine
            .receive_digest_challenge(
                malformed,
                Duration::from_millis(1),
                &credentials,
                None,
                None,
            )
            .unwrap_err();
        assert!(matches!(
            error,
            EngineError::Digest(DigestError::MissingParameter("nonce"))
        ));
        assert!(!format!("{error:?}").contains("atomic-password"));
        assert_eq!(engine.transaction_count(), transaction_count);
        assert!(!engine.digest_retries.contains_key(&call_id));
        assert_eq!(
            engine.snapshot(&call_id).unwrap().state,
            CallState::Inviting
        );

        let qop_challenge = digest_challenge(
            &request,
            401,
            "WWW-Authenticate",
            r#"Digest realm="carrier", nonce="nonce", qop="auth""#,
        );
        assert!(matches!(
            engine.receive_digest_challenge(
                qop_challenge.clone(),
                Duration::from_millis(2),
                &credentials,
                None,
                Some(1),
            ),
            Err(EngineError::Digest(DigestError::MissingParameter("cnonce")))
        ));
        assert_eq!(engine.transaction_count(), transaction_count);
        assert!(!engine.digest_retries.contains_key(&call_id));

        let valid = engine
            .receive_digest_challenge(
                qop_challenge,
                Duration::from_millis(3),
                &credentials,
                Some("atomic-cnonce"),
                Some(1),
            )
            .unwrap();
        assert_eq!(valid.actions().len(), 2);
        assert_eq!(engine.digest_retries.get(&call_id), Some(&1));
    }

    #[test]
    fn resolved_digest_credentials_are_lazy_and_missing_values_are_atomic() {
        let mut engine = CallEngine::new(EngineConfig::default()).unwrap();
        let request = invite(
            "digest-resolved",
            "sip-call-digest-resolved",
            "Bob <sip:bob@example.com>",
            "4 INVITE",
        );
        let (call_id, _) = engine
            .originate(
                request.clone(),
                address(5061),
                Duration::ZERO,
                TransportReliability::Unreliable,
            )
            .unwrap();
        let challenge = digest_challenge(
            &request,
            401,
            "WWW-Authenticate",
            r#"Digest realm="carrier", nonce="resolved-nonce", qop="auth""#,
        );
        let transaction_count = engine.transaction_count();

        assert_eq!(
            engine
                .receive_digest_challenge_resolved(
                    challenge.clone(),
                    Duration::from_millis(1),
                    Some("resolved-cnonce"),
                    Some(1),
                    || Err(EngineError::DigestCredentialsUnavailable),
                )
                .unwrap_err(),
            EngineError::DigestCredentialsUnavailable
        );
        assert_eq!(engine.transaction_count(), transaction_count);
        assert!(!engine.digest_retries.contains_key(&call_id));

        let resolutions = std::cell::Cell::new(0);
        let retried = engine
            .receive_digest_challenge_resolved(
                challenge.clone(),
                Duration::from_millis(2),
                Some("resolved-cnonce"),
                Some(1),
                || {
                    resolutions.set(resolutions.get() + 1);
                    Ok(DigestCredentials::new("resolved-user", "resolved-secret"))
                },
            )
            .unwrap();
        assert_eq!(resolutions.get(), 1);
        assert_eq!(retried.actions().len(), 2);

        let duplicate = engine
            .receive_digest_challenge_resolved(
                challenge,
                Duration::from_millis(3),
                None,
                None,
                || panic!("duplicate challenge must not resolve credentials"),
            )
            .unwrap();
        assert_eq!(duplicate.actions().len(), 1);
        assert_eq!(resolutions.get(), 1);
        assert!(!format!("{engine:?}").contains("resolved-secret"));
    }

    #[test]
    fn bye_reclaims_dialog_and_ends_call() {
        let mut engine = CallEngine::new(EngineConfig::default()).unwrap();
        let request = invite(
            "in-3",
            "sip-call-3",
            "Bob <sip:bob@example.com>",
            "1 INVITE",
        );
        let source = address(5060);
        let created = engine
            .receive_request(
                source,
                request.clone(),
                Duration::ZERO,
                TransportReliability::Unreliable,
            )
            .unwrap();
        let call_id = created.events()[0].call_id.clone();
        let answered = engine
            .respond_to_invite(&call_id, 200, "OK", Vec::new(), Duration::ZERO)
            .unwrap();
        let response = match &answered.actions()[0].message {
            SipMessage::Response(response) => response,
            _ => panic!("expected response"),
        };
        engine
            .receive_request(
                source,
                ack_for(&request, response, "ack-3"),
                Duration::ZERO,
                TransportReliability::Unreliable,
            )
            .unwrap();

        let mut headers = Headers::new();
        headers.push("Via", "SIP/2.0/UDP peer.invalid;branch=bye-1");
        headers.push("From", request.headers.get("From").unwrap());
        headers.push("To", response.headers.get("To").unwrap());
        headers.push("Call-ID", "sip-call-3");
        headers.push("CSeq", "2 BYE");
        let bye = SipRequest {
            method: SipMethod::Bye,
            request_uri: "sip:bob@example.com".to_owned(),
            version: "SIP/2.0".to_owned(),
            headers,
            body: Vec::new(),
        };
        let output = engine
            .receive_request(
                source,
                bye,
                Duration::ZERO,
                TransportReliability::Unreliable,
            )
            .unwrap();
        assert!(output.actions().iter().any(|action| matches!(
            action.message,
            SipMessage::Response(SipResponse {
                status_code: 200,
                ..
            })
        )));
        assert_eq!(engine.snapshot(&call_id).unwrap().state, CallState::Ended);
    }

    #[test]
    fn cancel_shares_invite_branch_without_being_misclassified_as_retransmission() {
        let mut engine = CallEngine::new(EngineConfig::default()).unwrap();
        let invite = invite(
            "shared-branch",
            "sip-call-4",
            "Bob <sip:bob@example.com>",
            "1 INVITE",
        );
        let source = address(5060);
        let initial = engine
            .receive_request(
                source,
                invite.clone(),
                Duration::ZERO,
                TransportReliability::Unreliable,
            )
            .unwrap();
        let call_id = initial.events()[0].call_id.clone();

        let retransmission = engine
            .receive_request(
                source,
                invite.clone(),
                Duration::ZERO,
                TransportReliability::Unreliable,
            )
            .unwrap();
        assert_eq!(retransmission.actions().len(), 1);
        assert!(matches!(
            retransmission.actions()[0].message,
            SipMessage::Response(SipResponse {
                status_code: 100,
                ..
            })
        ));
        assert_eq!(engine.list(10).unwrap().len(), 1);

        let mut headers = Headers::new();
        headers.push("Via", "SIP/2.0/UDP peer.invalid;branch=shared-branch");
        headers.push("From", invite.headers.get("From").unwrap());
        headers.push("To", invite.headers.get("To").unwrap());
        headers.push("Call-ID", "sip-call-4");
        headers.push("CSeq", "1 CANCEL");
        let cancel = SipRequest {
            method: SipMethod::Cancel,
            request_uri: invite.request_uri.clone(),
            version: "SIP/2.0".to_owned(),
            headers,
            body: Vec::new(),
        };
        let output = engine
            .receive_request(
                source,
                cancel,
                Duration::ZERO,
                TransportReliability::Unreliable,
            )
            .unwrap();
        assert_eq!(output.actions().len(), 2);
        assert!(output.actions().iter().any(|action| matches!(
            action.message,
            SipMessage::Response(SipResponse {
                status_code: 200,
                ..
            })
        )));
        assert!(output.actions().iter().any(|action| matches!(
            action.message,
            SipMessage::Response(SipResponse {
                status_code: 487,
                ..
            })
        )));
        assert_eq!(engine.snapshot(&call_id).unwrap().state, CallState::Ended);
    }

    #[test]
    fn final_invite_responses_replay_ack_and_non_2xx_ack_reuses_transaction_branch() {
        let mut engine = CallEngine::new(EngineConfig::default()).unwrap();
        let request = invite(
            "out-final",
            "sip-call-final",
            "Bob <sip:bob@example.com>",
            "7 INVITE",
        );
        let destination = address(5061);
        let (call_id, _) = engine
            .originate(
                request.clone(),
                destination,
                Duration::ZERO,
                TransportReliability::Unreliable,
            )
            .unwrap();

        let mut headers = Headers::new();
        headers.push("Via", "SIP/2.0/UDP peer.invalid;branch=out-final");
        headers.push("From", request.headers.get("From").unwrap());
        headers.push("To", "Bob <sip:bob@example.com>;tag=remote-final");
        headers.push("Call-ID", "sip-call-final");
        headers.push("CSeq", "7 INVITE");
        headers.push("Contact", "<sip:bob@127.0.0.1:5070>");
        let response = SipResponse {
            version: "SIP/2.0".to_owned(),
            status_code: 486,
            reason: "Busy Here".to_owned(),
            headers,
            body: Vec::new(),
        };
        let first = engine
            .receive_response(response.clone(), Duration::ZERO)
            .unwrap();
        let SipMessage::Request(first_ack) = &first.actions()[0].message else {
            panic!("expected ACK");
        };
        assert!(
            first_ack
                .headers
                .get("Via")
                .unwrap()
                .contains("branch=out-final")
        );
        assert_eq!(engine.snapshot(&call_id).unwrap().state, CallState::Ended);

        let duplicate = engine
            .receive_response(response, Duration::from_millis(1))
            .unwrap();
        assert!(matches!(
            duplicate.actions()[0].message,
            SipMessage::Request(SipRequest {
                method: SipMethod::Ack,
                ..
            })
        ));
    }

    #[test]
    fn transaction_timeout_fails_call_and_reclaims_transaction() {
        let mut engine = CallEngine::new(EngineConfig::default()).unwrap();
        let request = invite(
            "timeout-1",
            "sip-call-timeout",
            "Bob <sip:bob@example.com>",
            "1 INVITE",
        );
        let (call_id, _) = engine
            .originate(
                request,
                address(5061),
                Duration::ZERO,
                TransportReliability::Unreliable,
            )
            .unwrap();
        let output = engine.poll(Duration::from_secs(33)).unwrap();
        assert!(
            output
                .events()
                .iter()
                .any(|event| event.kind == CallEventKind::Failed)
        );
        assert_eq!(engine.transaction_count(), 0);
        assert_eq!(engine.snapshot(&call_id).unwrap().state, CallState::Ended);
    }

    #[test]
    fn options_are_transactional_and_retransmissions_replay_the_response() {
        let mut engine = CallEngine::new(EngineConfig::default()).unwrap();
        let request = options("options-1", "sip-options-1");
        let source = address(5060);
        let first = engine
            .receive_request(
                source,
                request.clone(),
                Duration::ZERO,
                TransportReliability::Unreliable,
            )
            .unwrap();
        assert!(matches!(
            first.actions()[0].message,
            SipMessage::Response(SipResponse {
                status_code: 200,
                ..
            })
        ));
        assert_eq!(engine.transaction_count(), 1);
        let duplicate = engine
            .receive_request(
                source,
                request,
                Duration::from_millis(1),
                TransportReliability::Unreliable,
            )
            .unwrap();
        assert!(matches!(
            duplicate.actions()[0].message,
            SipMessage::Response(SipResponse {
                status_code: 200,
                ..
            })
        ));
        engine.poll(Duration::from_secs(33)).unwrap();
        assert_eq!(engine.transaction_count(), 0);
    }

    #[test]
    fn malformed_cseq_is_rejected_before_call_creation() {
        let mut engine = CallEngine::new(EngineConfig::default()).unwrap();
        let request = invite(
            "bad-cseq",
            "sip-call-bad-cseq",
            "Bob <sip:bob@example.com>",
            "not-a-number INVITE",
        );
        assert_eq!(
            engine
                .receive_request(
                    address(5060),
                    request,
                    Duration::ZERO,
                    TransportReliability::Unreliable,
                )
                .unwrap_err(),
            EngineError::InvalidCSeq
        );
        assert!(engine.list(1).unwrap().is_empty());
    }

    #[test]
    fn invalid_dialog_and_timer_bounds_are_rejected_at_engine_creation() {
        let invalid_dialog = EngineConfig {
            dialog: DialogConfig {
                max_field_bytes: 0,
                ..DialogConfig::default()
            },
            ..EngineConfig::default()
        };
        assert_eq!(
            CallEngine::new(invalid_dialog).unwrap_err(),
            EngineError::InvalidConfig
        );
        let invalid_timers = EngineConfig {
            timers: TimerConfig {
                t1: Duration::from_secs(2),
                t2: Duration::from_secs(1),
                ..TimerConfig::default()
            },
            ..EngineConfig::default()
        };
        assert_eq!(
            CallEngine::new(invalid_timers).unwrap_err(),
            EngineError::InvalidConfig
        );
        let invalid_digest_retries = EngineConfig {
            max_digest_retries_per_call: 0,
            ..EngineConfig::default()
        };
        assert_eq!(
            CallEngine::new(invalid_digest_retries).unwrap_err(),
            EngineError::InvalidConfig
        );
    }

    #[test]
    fn transaction_limit_is_enforced_without_mutating_existing_call() {
        let config = EngineConfig {
            max_transactions: 1,
            ..EngineConfig::default()
        };
        let mut engine = CallEngine::new(config).unwrap();
        let first = invite(
            "limit-1",
            "sip-call-limit-1",
            "Bob <sip:bob@example.com>",
            "1 INVITE",
        );
        let initial = engine
            .receive_request(
                address(5060),
                first,
                Duration::ZERO,
                TransportReliability::Unreliable,
            )
            .unwrap();
        let call_id = initial.events()[0].call_id.clone();
        let second = invite(
            "limit-2",
            "sip-call-limit-2",
            "Bob <sip:bob@example.com>",
            "1 INVITE",
        );
        assert_eq!(
            engine
                .receive_request(
                    address(5060),
                    second,
                    Duration::ZERO,
                    TransportReliability::Unreliable,
                )
                .unwrap_err(),
            EngineError::TransactionLimitReached
        );
        assert_eq!(engine.list(10).unwrap().len(), 1);
        assert_eq!(
            engine.snapshot(&call_id).unwrap().state,
            CallState::Inviting
        );
    }

    #[test]
    fn malformed_final_response_does_not_consume_outbound_transaction() {
        let mut engine = CallEngine::new(EngineConfig::default()).unwrap();
        let request = invite(
            "atomic-response",
            "sip-call-atomic-response",
            "Bob <sip:bob@example.com>",
            "1 INVITE",
        );
        let (call_id, _) = engine
            .originate(
                request.clone(),
                address(5061),
                Duration::ZERO,
                TransportReliability::Unreliable,
            )
            .unwrap();
        let mut headers = Headers::new();
        headers.push("Via", "SIP/2.0/UDP peer.invalid;branch=atomic-response");
        headers.push("From", request.headers.get("From").unwrap());
        headers.push("Call-ID", "sip-call-atomic-response");
        headers.push("CSeq", "1 INVITE");
        let response = SipResponse {
            version: "SIP/2.0".to_owned(),
            status_code: 200,
            reason: "OK".to_owned(),
            headers,
            body: Vec::new(),
        };
        assert!(matches!(
            engine.receive_response(response, Duration::ZERO),
            Err(EngineError::Dialog(DialogError::MissingRemoteTag))
        ));
        assert_eq!(engine.transaction_count(), 1);
        assert_eq!(
            engine.snapshot(&call_id).unwrap().state,
            CallState::Inviting
        );
    }

    #[test]
    fn cancel_with_different_branch_cannot_terminate_invite() {
        let mut engine = CallEngine::new(EngineConfig::default()).unwrap();
        let request = invite(
            "invite-branch",
            "sip-call-cancel-branch",
            "Bob <sip:bob@example.com>",
            "1 INVITE",
        );
        let source = address(5060);
        let initial = engine
            .receive_request(
                source,
                request.clone(),
                Duration::ZERO,
                TransportReliability::Unreliable,
            )
            .unwrap();
        let call_id = initial.events()[0].call_id.clone();
        let mut headers = Headers::new();
        headers.push("Via", "SIP/2.0/UDP peer.invalid;branch=wrong-branch");
        headers.push("From", request.headers.get("From").unwrap());
        headers.push("To", request.headers.get("To").unwrap());
        headers.push("Call-ID", "sip-call-cancel-branch");
        headers.push("CSeq", "1 CANCEL");
        let cancel = SipRequest {
            method: SipMethod::Cancel,
            request_uri: request.request_uri,
            version: "SIP/2.0".to_owned(),
            headers,
            body: Vec::new(),
        };
        let output = engine
            .receive_request(
                source,
                cancel,
                Duration::ZERO,
                TransportReliability::Unreliable,
            )
            .unwrap();
        assert!(matches!(
            output.actions()[0].message,
            SipMessage::Response(SipResponse {
                status_code: 481,
                ..
            })
        ));
        assert_eq!(
            engine.snapshot(&call_id).unwrap().state,
            CallState::Inviting
        );
    }
}
