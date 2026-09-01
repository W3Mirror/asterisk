//! Bounded internal call-control API and lifecycle event registry.

use std::{
    collections::{HashMap, VecDeque},
    error::Error,
    fmt::{Display, Formatter},
};

use call_core::{Call, CallEventKind, CallId, CallState, CommandId, EventId, LifecycleEvent};
use sdp::{Codec, Direction, SessionDescription};
use sip_dialog::DialogId;

const DEFAULT_MAX_CALLS: usize = 4_096;
const DEFAULT_MAX_PENDING_EVENTS: usize = 16_384;
const DEFAULT_MAX_COMMAND_KEYS: usize = 4_096;
const MAX_PRINCIPAL_ID_BYTES: usize = 128;

/// Permission granted to an authenticated control-plane principal.
///
/// The SIP transport and its internally generated lifecycle commands remain a
/// trusted engine boundary. These permissions govern application-originated
/// calls into the control API.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ControlPermission {
    /// Read call snapshots and retained lifecycle events.
    ReadCalls,
    /// Start a new outbound call or mark a call as invited.
    OriginateCalls,
    /// Advance a call through ringing, answer, early media, or active media.
    ManageCalls,
    /// Begin or complete a transfer.
    TransferCalls,
    /// Hang up, fail, end, or reclaim a call.
    HangupCalls,
    /// Bypass command-specific permissions for an explicitly trusted operator.
    Admin,
}

impl ControlPermission {
    /// Returns the stable machine-readable permission name.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::ReadCalls => "calls:read",
            Self::OriginateCalls => "calls:originate",
            Self::ManageCalls => "calls:manage",
            Self::TransferCalls => "calls:transfer",
            Self::HangupCalls => "calls:hangup",
            Self::Admin => "calls:admin",
        }
    }

    const fn bit(self) -> u16 {
        1 << (self as u16)
    }
}

/// Identity and verified permissions handed off by an outer authentication
/// adapter.
///
/// This type intentionally stores only a bounded, non-secret principal ID and
/// permission bits. Bearer tokens, passwords, signatures, and verification
/// keys must remain in the adapter that authenticates the request. Construct
/// one only after that adapter has verified its credentials and claims.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthenticatedPrincipal {
    id: String,
    permissions: u16,
}

impl AuthenticatedPrincipal {
    /// Creates a principal from claims already verified by an outer adapter.
    ///
    /// The ID is bounded and restricted to printable ASCII so it can safely be
    /// used in audit fields. Permission values are represented as a fixed bit
    /// set, so duplicate claims never increase memory usage.
    pub fn from_verified_claims(
        id: impl Into<String>,
        permissions: impl IntoIterator<Item = ControlPermission>,
    ) -> Result<Self, ApiError> {
        let id = id.into();
        if id.is_empty()
            || id.len() > MAX_PRINCIPAL_ID_BYTES
            || !id.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(ApiError::InvalidPrincipal);
        }
        let permissions = permissions
            .into_iter()
            .fold(0_u16, |bits, permission| bits | permission.bit());
        Ok(Self { id, permissions })
    }

    /// Returns the stable non-secret principal ID.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns whether this principal has one permission.
    #[must_use]
    pub fn has_permission(&self, permission: ControlPermission) -> bool {
        self.permissions & permission.bit() != 0
    }

    /// Authorizes one application-originated lifecycle command.
    pub fn authorize(&self, command: CallCommand) -> Result<(), ApiError> {
        let permission = required_permission(command);
        if self.has_permission(permission) || self.has_permission(ControlPermission::Admin) {
            Ok(())
        } else {
            Err(ApiError::PermissionDenied {
                command,
                permission,
            })
        }
    }

    /// Authorizes a read of call state or retained lifecycle events.
    pub fn authorize_read(&self) -> Result<(), ApiError> {
        if self.has_permission(ControlPermission::ReadCalls)
            || self.has_permission(ControlPermission::Admin)
        {
            Ok(())
        } else {
            Err(ApiError::ReadPermissionDenied)
        }
    }
}

/// Limits for the in-memory call-control registry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CallRegistryConfig {
    /// Maximum number of active call records retained at once.
    pub max_calls: usize,
    /// Maximum number of lifecycle events retained until drained.
    pub max_pending_events: usize,
    /// Maximum number of idempotency keys retained for command retries.
    pub max_command_keys: usize,
}

impl Default for CallRegistryConfig {
    fn default() -> Self {
        Self {
            max_calls: DEFAULT_MAX_CALLS,
            max_pending_events: DEFAULT_MAX_PENDING_EVENTS,
            max_command_keys: DEFAULT_MAX_COMMAND_KEYS,
        }
    }
}

impl CallRegistryConfig {
    fn validate(self) -> Result<Self, ApiError> {
        if self.max_calls == 0 || self.max_pending_events == 0 || self.max_command_keys == 0 {
            return Err(ApiError::InvalidConfig);
        }
        Ok(self)
    }
}

/// Internal call-control operations corresponding to the lifecycle contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CallCommand {
    /// A new inbound or outbound INVITE is being attempted.
    InviteReceived,
    /// Early media became available.
    EarlyMedia,
    /// The remote side is ringing.
    Ringing,
    /// The call was answered.
    Answer,
    /// Media forwarding started.
    MediaStarted,
    /// A transfer was requested.
    BeginTransfer,
    /// A transfer completed and the active leg resumed.
    CompleteTransfer,
    /// Hang up and begin terminal cleanup.
    Hangup,
    /// Finish cleanup after hangup or failure.
    End,
    /// Mark the call as failed.
    Fail,
}

fn required_permission(command: CallCommand) -> ControlPermission {
    match command {
        CallCommand::InviteReceived => ControlPermission::OriginateCalls,
        CallCommand::EarlyMedia
        | CallCommand::Ringing
        | CallCommand::Answer
        | CallCommand::MediaStarted => ControlPermission::ManageCalls,
        CallCommand::BeginTransfer | CallCommand::CompleteTransfer => {
            ControlPermission::TransferCalls
        }
        CallCommand::Hangup | CallCommand::End | CallCommand::Fail => {
            ControlPermission::HangupCalls
        }
    }
}

/// A bridge-control operation represented in the bounded audit trail.
///
/// Bridge state is owned by the runtime bridge registry rather than the call
/// registry, but the audit trail deliberately keeps one stable operation
/// vocabulary for all application-originated control-plane mutations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BridgeControlOperation {
    /// Start an outbound human second leg for a bridge.
    OriginateHuman,
    /// Promote a pending human leg to the active destination.
    CompleteHuman,
    /// Fail a human leg and restore AI routing.
    FailHuman,
    /// Restore AI routing from an active human leg.
    ResumeAi,
    /// End bridge forwarding and release its retained endpoints.
    EndBridge,
}

/// A control-plane operation represented in the bounded audit trail.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuditOperation {
    /// Apply a lifecycle command, optionally using an idempotency key.
    ApplyCommand {
        /// Lifecycle command requested by the caller.
        command: CallCommand,
        /// Whether the command was submitted through the idempotent API.
        idempotent: bool,
    },
    /// Start an outbound call.
    Originate,
    /// Respond to an inbound INVITE.
    RespondToInvite,
    /// Reclaim terminal call resources.
    ReclaimTerminal,
    /// Negotiate or update the call's audio media.
    NegotiateAudio,
    /// Send an application-controlled in-dialog SIP request.
    InDialogRequest,
    /// Apply an authorized bridge-control operation.
    BridgeControl {
        /// Bridge operation requested by the caller.
        operation: BridgeControlOperation,
    },
}

/// Outcome recorded for one control-plane audit operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuditOutcome {
    /// The operation completed and changed or read the requested state.
    Succeeded,
    /// An idempotent retry returned its retained result.
    Replayed,
    /// The operation was rejected; the value is a stable [`ApiError::code`].
    Rejected(&'static str),
}

/// One bounded, credential-free control-plane audit record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditRecord {
    /// Monotonic sequence within the lifetime of the registry.
    pub sequence: u64,
    /// Verified, non-secret principal identifier.
    pub principal_id: String,
    /// Application call identifier supplied to or produced by the operation.
    pub call_id: Option<CallId>,
    /// Operation that was requested.
    pub operation: AuditOperation,
    /// Result visible to the control-plane caller.
    pub outcome: AuditOutcome,
}

/// A stable, read-only view of a registered call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallSnapshot {
    /// Application-owned call identifier.
    pub id: CallId,
    /// Current high-level lifecycle state.
    pub state: CallState,
    /// Optional tag-qualified SIP dialog identity.
    pub dialog_id: Option<DialogId>,
    /// Current negotiated audio media, if an offer/answer has completed.
    pub media: Option<NegotiatedAudio>,
}

/// Result of applying a call command with an idempotency key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandResult {
    /// Lifecycle event produced by the command, if any.
    pub event: Option<LifecycleEvent>,
    /// Whether the result was replayed from the bounded idempotency store.
    pub replayed: bool,
}

/// Bounded, cardinality-safe call-control metrics.
///
/// Counters are cumulative for the lifetime of a registry. Gauges describe
/// the currently retained in-memory state. No call, SIP, provider, principal,
/// or credential identifiers are included, so this snapshot can be exported
/// without creating unbounded metric-label cardinality or leaking secrets.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CallMetrics {
    /// Number of calls registered since the registry was created.
    pub calls_started_total: u64,
    /// Number of calls that reached the answered state.
    pub calls_answered_total: u64,
    /// Number of calls marked failed.
    pub calls_failed_total: u64,
    /// Number of calls that reached the ended state.
    pub calls_completed_total: u64,
    /// Number of non-terminal calls currently retained.
    pub calls_active: usize,
    /// Number of call records currently retained, including terminal calls
    /// awaiting explicit reclamation.
    pub calls_retained: usize,
    /// Number of lifecycle events emitted since the registry was created.
    pub lifecycle_events_total: u64,
    /// Number of events waiting for delivery.
    pub pending_events: usize,
    /// Number of events available for bounded replay.
    pub retained_event_history: usize,
    /// Number of idempotency keys currently retained.
    pub retained_command_keys: usize,
    /// Number of audit records waiting to be drained.
    pub pending_audit_records: usize,
}

/// The bounded audio offer/answer result retained for a call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NegotiatedAudio {
    /// Codec selected from the local description.
    pub local_codec: Codec,
    /// Matching codec selected from the remote description.
    pub remote_codec: Codec,
    /// Direction of the resulting local media stream.
    pub direction: Direction,
    /// Remote connection attribute, when one was advertised.
    pub remote_connection: Option<String>,
    /// Remote RTP port advertised by the negotiated audio section.
    pub remote_port: u16,
}

/// Errors exposed by the internal call-control API.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApiError {
    /// A registry bound was zero.
    InvalidConfig,
    /// A verified principal ID was empty, too large, or not printable ASCII.
    InvalidPrincipal,
    /// The principal lacks the permission required by a command.
    PermissionDenied {
        /// Rejected lifecycle command.
        command: CallCommand,
        /// Permission required to apply the command.
        permission: ControlPermission,
    },
    /// The principal lacks permission to read call state or events.
    ReadPermissionDenied,
    /// The active-call bound was reached.
    CallLimitReached,
    /// A supplied call identifier is already registered.
    DuplicateCall,
    /// The requested call does not exist.
    UnknownCall,
    /// The pending event bound was reached.
    EventQueueFull,
    /// The requested replay cursor is no longer retained in the bounded history.
    EventHistoryUnavailable,
    /// An idempotency key was reused for a different call or command.
    IdempotencyConflict,
    /// A caller supplied a zero event/list limit.
    InvalidLimit,
    /// The requested command is not valid in the current state.
    InvalidCommand {
        /// Current call state.
        state: CallState,
        /// Rejected lifecycle command.
        command: CallCommand,
    },
    /// The generated identifier sequence cannot advance safely.
    IdentifierExhausted,
    /// A call already has a bound SIP dialog.
    DialogAlreadyBound,
    /// Neither SDP description contains an audio media section.
    NoAudioMedia,
    /// The descriptions do not share a usable audio codec.
    NoCommonCodec,
    /// The remote description rejected its audio media section.
    MediaRejected,
}

impl ApiError {
    /// Returns a stable machine-readable error code.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidConfig => "invalid_config",
            Self::InvalidPrincipal => "invalid_principal",
            Self::PermissionDenied { .. } => "permission_denied",
            Self::ReadPermissionDenied => "read_permission_denied",
            Self::CallLimitReached => "call_limit_reached",
            Self::DuplicateCall => "duplicate_call",
            Self::UnknownCall => "unknown_call",
            Self::EventQueueFull => "event_queue_full",
            Self::EventHistoryUnavailable => "event_history_unavailable",
            Self::IdempotencyConflict => "idempotency_conflict",
            Self::InvalidLimit => "invalid_limit",
            Self::InvalidCommand { .. } => "invalid_command",
            Self::IdentifierExhausted => "identifier_exhausted",
            Self::DialogAlreadyBound => "dialog_already_bound",
            Self::NoAudioMedia => "no_audio_media",
            Self::NoCommonCodec => "no_common_codec",
            Self::MediaRejected => "media_rejected",
        }
    }
}

impl Display for ApiError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfig => formatter.write_str("call registry bounds must be non-zero"),
            Self::InvalidPrincipal => formatter
                .write_str("principal ID must be non-empty printable ASCII within 128 bytes"),
            Self::PermissionDenied {
                command,
                permission,
            } => {
                write!(
                    formatter,
                    "principal lacks {} for command {command:?}",
                    permission.code()
                )
            }
            Self::ReadPermissionDenied => {
                formatter.write_str("principal lacks calls:read permission")
            }
            Self::CallLimitReached => formatter.write_str("call registry reached its call limit"),
            Self::DuplicateCall => formatter.write_str("call identifier is already registered"),
            Self::UnknownCall => formatter.write_str("call identifier is not registered"),
            Self::EventQueueFull => formatter.write_str("call lifecycle event queue is full"),
            Self::EventHistoryUnavailable => {
                formatter.write_str("call lifecycle event cursor is outside the retained history")
            }
            Self::IdempotencyConflict => {
                formatter.write_str("idempotency key was already used for a different command")
            }
            Self::InvalidLimit => formatter.write_str("call API limit must be non-zero"),
            Self::InvalidCommand { state, command } => {
                write!(
                    formatter,
                    "command {command:?} is invalid in state {state:?}"
                )
            }
            Self::IdentifierExhausted => {
                formatter.write_str("call or event identifier sequence exhausted")
            }
            Self::DialogAlreadyBound => formatter.write_str("call already has a SIP dialog bound"),
            Self::NoAudioMedia => formatter.write_str("SDP descriptions contain no audio media"),
            Self::NoCommonCodec => {
                formatter.write_str("SDP descriptions have no common audio codec")
            }
            Self::MediaRejected => formatter.write_str("remote SDP rejected its audio media"),
        }
    }
}

impl Error for ApiError {}

#[derive(Clone, Debug)]
struct CallEntry {
    call: Call,
    dialog_id: Option<DialogId>,
    media: Option<NegotiatedAudio>,
}

#[derive(Clone, Debug)]
struct AppliedCommand {
    call_id: CallId,
    command: CallCommand,
    event: Option<LifecycleEvent>,
}

/// A bounded registry that composes call state, SIP dialogs, and lifecycle
/// events without owning sockets, transactions, or an async runtime.
#[derive(Clone, Debug)]
pub struct CallRegistry {
    config: CallRegistryConfig,
    calls: HashMap<CallId, CallEntry>,
    events: VecDeque<LifecycleEvent>,
    event_history: VecDeque<LifecycleEvent>,
    applied_commands: HashMap<CommandId, AppliedCommand>,
    command_order: VecDeque<CommandId>,
    audit_records: VecDeque<AuditRecord>,
    calls_started_total: u64,
    calls_answered_total: u64,
    calls_failed_total: u64,
    calls_completed_total: u64,
    lifecycle_events_total: u64,
    next_call_sequence: u64,
    next_event_sequence: u64,
    next_audit_sequence: u64,
}

impl CallRegistry {
    /// Creates an empty registry with validated resource bounds.
    pub fn new(config: CallRegistryConfig) -> Result<Self, ApiError> {
        Ok(Self {
            config: config.validate()?,
            calls: HashMap::new(),
            events: VecDeque::new(),
            event_history: VecDeque::new(),
            applied_commands: HashMap::new(),
            command_order: VecDeque::new(),
            audit_records: VecDeque::new(),
            calls_started_total: 0,
            calls_answered_total: 0,
            calls_failed_total: 0,
            calls_completed_total: 0,
            lifecycle_events_total: 0,
            next_call_sequence: 1,
            next_event_sequence: 1,
            next_audit_sequence: 1,
        })
    }

    /// Returns the configured resource bounds.
    #[must_use]
    pub fn config(&self) -> CallRegistryConfig {
        self.config
    }

    /// Returns bounded lifecycle counters and current queue gauges.
    #[must_use]
    pub fn metrics(&self) -> CallMetrics {
        CallMetrics {
            calls_started_total: self.calls_started_total,
            calls_answered_total: self.calls_answered_total,
            calls_failed_total: self.calls_failed_total,
            calls_completed_total: self.calls_completed_total,
            calls_active: self
                .calls
                .values()
                .filter(|entry| !matches!(entry.call.state, CallState::Ended | CallState::Failed))
                .count(),
            calls_retained: self.calls.len(),
            lifecycle_events_total: self.lifecycle_events_total,
            pending_events: self.events.len(),
            retained_event_history: self.event_history.len(),
            retained_command_keys: self.applied_commands.len(),
            pending_audit_records: self.audit_records.len(),
        }
    }

    /// Registers a generated application call identifier and emits `Created`.
    pub fn create(&mut self) -> Result<CallId, ApiError> {
        if self.calls.len() >= self.config.max_calls {
            return Err(ApiError::CallLimitReached);
        }
        let event_id = self.reserve_event_id()?;
        let id = self.allocate_call_id()?;
        self.insert_created_call(id.clone());
        self.commit_event(event_id, id.clone(), CallEventKind::Created);
        Ok(id)
    }

    /// Registers a caller-supplied application identifier and emits `Created`.
    pub fn create_with_id(&mut self, id: CallId) -> Result<CallId, ApiError> {
        if self.calls.contains_key(&id) {
            return Err(ApiError::DuplicateCall);
        }
        self.create_with_id_inner(id)
    }

    fn create_with_id_inner(&mut self, id: CallId) -> Result<CallId, ApiError> {
        if self.calls.len() >= self.config.max_calls {
            return Err(ApiError::CallLimitReached);
        }
        let event_id = self.reserve_event_id()?;
        self.insert_created_call(id.clone());
        self.commit_event(event_id, id.clone(), CallEventKind::Created);
        Ok(id)
    }

    fn insert_created_call(&mut self, id: CallId) {
        self.calls_started_total = self.calls_started_total.saturating_add(1);
        self.calls.insert(
            id.clone(),
            CallEntry {
                call: Call::new(id),
                dialog_id: None,
                media: None,
            },
        );
    }

    /// Applies one lifecycle command and emits its event, if it has one.
    pub fn apply(
        &mut self,
        id: &CallId,
        command: CallCommand,
    ) -> Result<Option<LifecycleEvent>, ApiError> {
        let state = self.calls.get(id).ok_or(ApiError::UnknownCall)?.call.state;
        let (next_state, event_kind) = command_transition(state, command);
        if next_state == state {
            return Err(ApiError::InvalidCommand { state, command });
        }
        let event_id = event_kind.map(|_| self.reserve_event_id()).transpose()?;
        let entry = self.calls.get_mut(id).ok_or(ApiError::UnknownCall)?;
        entry
            .call
            .transition(next_state)
            .map_err(|_| ApiError::InvalidCommand { state, command })?;
        match command {
            CallCommand::Answer => {
                self.calls_answered_total = self.calls_answered_total.saturating_add(1);
            }
            CallCommand::Fail => {
                self.calls_failed_total = self.calls_failed_total.saturating_add(1);
            }
            CallCommand::End => {
                self.calls_completed_total = self.calls_completed_total.saturating_add(1);
            }
            _ => {}
        }
        Ok(match event_kind {
            Some(kind) => Some(self.commit_event(
                event_id.ok_or(ApiError::IdentifierExhausted)?,
                id.clone(),
                kind,
            )),
            None => None,
        })
    }

    /// Applies one lifecycle command after verifying the caller's permission.
    ///
    /// Authorization is checked before call lookup so a caller without the
    /// required permission cannot use this method to probe call existence.
    pub fn apply_authorized(
        &mut self,
        principal: &AuthenticatedPrincipal,
        id: &CallId,
        command: CallCommand,
    ) -> Result<Option<LifecycleEvent>, ApiError> {
        let operation = AuditOperation::ApplyCommand {
            command,
            idempotent: false,
        };
        if let Err(error) = principal.authorize(command) {
            self.record_audit(
                principal,
                Some(id),
                operation,
                AuditOutcome::Rejected(error.code()),
            );
            return Err(error);
        }
        let result = self.apply(id, command);
        match &result {
            Ok(_) => self.record_audit(principal, Some(id), operation, AuditOutcome::Succeeded),
            Err(error) => self.record_audit(
                principal,
                Some(id),
                operation,
                AuditOutcome::Rejected(error.code()),
            ),
        }
        result
    }

    /// Applies a command exactly once for a bounded idempotency key.
    ///
    /// A retry with the same key and identical call/command returns the
    /// original result without mutating state or emitting a duplicate event.
    /// Reusing a retained key for a different call or command is rejected.
    pub fn apply_idempotent(
        &mut self,
        id: &CallId,
        command: CallCommand,
        command_id: CommandId,
    ) -> Result<CommandResult, ApiError> {
        if let Some(applied) = self.applied_commands.get(&command_id) {
            if applied.call_id != *id || applied.command != command {
                return Err(ApiError::IdempotencyConflict);
            }
            return Ok(CommandResult {
                event: applied.event.clone(),
                replayed: true,
            });
        }

        let event = self.apply(id, command)?;
        if self.applied_commands.len() >= self.config.max_command_keys {
            if let Some(evicted) = self.command_order.pop_front() {
                self.applied_commands.remove(&evicted);
            }
        }
        self.command_order.push_back(command_id.clone());
        self.applied_commands.insert(
            command_id,
            AppliedCommand {
                call_id: id.clone(),
                command,
                event: event.clone(),
            },
        );
        Ok(CommandResult {
            event,
            replayed: false,
        })
    }

    /// Applies an idempotent lifecycle command after verifying permission.
    ///
    /// Authorization precedes idempotency lookup, so an unauthorized retry
    /// cannot replay a retained event or probe its key's history.
    pub fn apply_idempotent_authorized(
        &mut self,
        principal: &AuthenticatedPrincipal,
        id: &CallId,
        command: CallCommand,
        command_id: CommandId,
    ) -> Result<CommandResult, ApiError> {
        let operation = AuditOperation::ApplyCommand {
            command,
            idempotent: true,
        };
        if let Err(error) = principal.authorize(command) {
            self.record_audit(
                principal,
                Some(id),
                operation,
                AuditOutcome::Rejected(error.code()),
            );
            return Err(error);
        }
        let result = self.apply_idempotent(id, command, command_id);
        match &result {
            Ok(command_result) if command_result.replayed => {
                self.record_audit(principal, Some(id), operation, AuditOutcome::Replayed)
            }
            Ok(_) => self.record_audit(principal, Some(id), operation, AuditOutcome::Succeeded),
            Err(error) => self.record_audit(
                principal,
                Some(id),
                operation,
                AuditOutcome::Rejected(error.code()),
            ),
        }
        result
    }

    /// Associates a tag-qualified SIP dialog with a call exactly once.
    pub fn bind_dialog(&mut self, id: &CallId, dialog_id: DialogId) -> Result<(), ApiError> {
        let entry = self.calls.get_mut(id).ok_or(ApiError::UnknownCall)?;
        if entry.dialog_id.is_some() {
            return Err(ApiError::DialogAlreadyBound);
        }
        entry.dialog_id = Some(dialog_id);
        Ok(())
    }

    /// Negotiates and retains one bounded audio offer/answer result.
    ///
    /// A later invocation replaces the prior result, which allows callers to
    /// represent an SDP update such as a re-INVITE without creating another
    /// application call record.
    pub fn negotiate_audio(
        &mut self,
        id: &CallId,
        local: &SessionDescription,
        remote: &SessionDescription,
    ) -> Result<NegotiatedAudio, ApiError> {
        self.calls.get(id).ok_or(ApiError::UnknownCall)?;
        let local_media = local
            .media
            .iter()
            .find(|media| media.media.eq_ignore_ascii_case("audio"))
            .ok_or(ApiError::NoAudioMedia)?;
        let remote_media = remote
            .media
            .iter()
            .find(|media| media.media.eq_ignore_ascii_case("audio"))
            .ok_or(ApiError::NoAudioMedia)?;
        if remote_media.port == 0 {
            return Err(ApiError::MediaRejected);
        }
        let local_codec = local_media
            .codecs
            .iter()
            .find(|candidate| {
                local_media.formats.contains(&candidate.payload_type)
                    && !candidate.is_telephone_event()
                    && remote_media.codecs.iter().any(|remote_codec| {
                        remote_media.formats.contains(&remote_codec.payload_type)
                            && candidate.name.eq_ignore_ascii_case(&remote_codec.name)
                            && candidate.clock_rate == remote_codec.clock_rate
                            && candidate.channels == remote_codec.channels
                    })
            })
            .cloned()
            .ok_or(ApiError::NoCommonCodec)?;
        let remote_codec = remote_media
            .codecs
            .iter()
            .find(|candidate| {
                remote_media.formats.contains(&candidate.payload_type)
                    && local_media.formats.contains(&local_codec.payload_type)
                    && candidate.name.eq_ignore_ascii_case(&local_codec.name)
                    && candidate.clock_rate == local_codec.clock_rate
                    && candidate.channels == local_codec.channels
            })
            .cloned()
            .ok_or(ApiError::NoCommonCodec)?;
        let direction = Direction::negotiate(
            local_media.effective_direction(local.direction),
            remote_media.effective_direction(remote.direction),
        );
        let negotiated = NegotiatedAudio {
            local_codec,
            remote_codec,
            direction,
            remote_connection: remote_media
                .connection
                .clone()
                .or_else(|| remote.connection.clone()),
            remote_port: remote_media.port,
        };
        self.calls.get_mut(id).ok_or(ApiError::UnknownCall)?.media = Some(negotiated.clone());
        Ok(negotiated)
    }

    /// Returns the retained audio negotiation for one call, if present.
    pub fn media(&self, id: &CallId) -> Result<Option<NegotiatedAudio>, ApiError> {
        Ok(self
            .calls
            .get(id)
            .ok_or(ApiError::UnknownCall)?
            .media
            .clone())
    }

    /// Returns a stable snapshot of one call.
    pub fn snapshot(&self, id: &CallId) -> Result<CallSnapshot, ApiError> {
        let entry = self.calls.get(id).ok_or(ApiError::UnknownCall)?;
        Ok(CallSnapshot {
            id: entry.call.id.clone(),
            state: entry.call.state,
            dialog_id: entry.dialog_id.clone(),
            media: entry.media.clone(),
        })
    }

    /// Returns a call snapshot after verifying read permission.
    pub fn snapshot_authorized(
        &self,
        principal: &AuthenticatedPrincipal,
        id: &CallId,
    ) -> Result<CallSnapshot, ApiError> {
        principal.authorize_read()?;
        self.snapshot(id)
    }

    /// Returns up to `limit` snapshots in deterministic identifier order.
    pub fn list(&self, limit: usize) -> Result<Vec<CallSnapshot>, ApiError> {
        if limit == 0 {
            return Err(ApiError::InvalidLimit);
        }
        let mut snapshots = self
            .calls
            .values()
            .map(|entry| CallSnapshot {
                id: entry.call.id.clone(),
                state: entry.call.state,
                dialog_id: entry.dialog_id.clone(),
                media: entry.media.clone(),
            })
            .collect::<Vec<_>>();
        snapshots.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
        snapshots.truncate(limit);
        Ok(snapshots)
    }

    /// Lists call snapshots after verifying read permission.
    pub fn list_authorized(
        &self,
        principal: &AuthenticatedPrincipal,
        limit: usize,
    ) -> Result<Vec<CallSnapshot>, ApiError> {
        principal.authorize_read()?;
        self.list(limit)
    }

    /// Returns the number of events waiting to be delivered.
    #[must_use]
    pub fn pending_events(&self) -> usize {
        self.events.len()
    }

    /// Returns the number of audit records waiting to be drained.
    #[must_use]
    pub fn pending_audit_records(&self) -> usize {
        self.audit_records.len()
    }

    /// Drains up to `limit` audit records in emission order.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::InvalidLimit`] when `limit` is zero.
    pub fn drain_audit_records(&mut self, limit: usize) -> Result<Vec<AuditRecord>, ApiError> {
        if limit == 0 {
            return Err(ApiError::InvalidLimit);
        }
        Ok(self
            .audit_records
            .drain(..limit.min(self.audit_records.len()))
            .collect())
    }

    /// Drains audit records after verifying read permission.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::ReadPermissionDenied`] when the principal lacks
    /// `calls:read`, or [`ApiError::InvalidLimit`] when `limit` is zero.
    pub fn drain_audit_records_authorized(
        &mut self,
        principal: &AuthenticatedPrincipal,
        limit: usize,
    ) -> Result<Vec<AuditRecord>, ApiError> {
        principal.authorize_read()?;
        self.drain_audit_records(limit)
    }

    /// Drains up to `limit` lifecycle events in emission order.
    pub fn drain_events(&mut self, limit: usize) -> Result<Vec<LifecycleEvent>, ApiError> {
        if limit == 0 {
            return Err(ApiError::InvalidLimit);
        }
        Ok(self.events.drain(..limit.min(self.events.len())).collect())
    }

    /// Returns retained lifecycle events emitted after an optional cursor.
    ///
    /// The cursor is exclusive: passing the last event returned by a consumer
    /// yields only newer events. Events remain replayable after
    /// [`Self::drain_events`] so a reconnecting consumer can backfill without
    /// duplicating already acknowledged events. A cursor that has fallen out
    /// of the bounded history is rejected instead of silently returning a
    /// partial stream.
    pub fn replay_events_after(
        &self,
        after: Option<&EventId>,
        limit: usize,
    ) -> Result<Vec<LifecycleEvent>, ApiError> {
        if limit == 0 {
            return Err(ApiError::InvalidLimit);
        }
        let start = match after {
            None => 0,
            Some(cursor) => self
                .event_history
                .iter()
                .position(|event| &event.event_id == cursor)
                .map_or_else(
                    || Err(ApiError::EventHistoryUnavailable),
                    |position| Ok(position.saturating_add(1)),
                )?,
        };
        Ok(self
            .event_history
            .iter()
            .skip(start)
            .take(limit)
            .cloned()
            .collect())
    }

    /// Replays retained lifecycle events after verifying read permission.
    pub fn replay_events_after_authorized(
        &self,
        principal: &AuthenticatedPrincipal,
        after: Option<&EventId>,
        limit: usize,
    ) -> Result<Vec<LifecycleEvent>, ApiError> {
        principal.authorize_read()?;
        self.replay_events_after(after, limit)
    }

    /// Returns the newest retained lifecycle event identifier, if any.
    #[must_use]
    pub fn latest_event_id(&self) -> Option<&EventId> {
        self.event_history.back().map(|event| &event.event_id)
    }

    /// Returns the oldest retained lifecycle event identifier, if any.
    #[must_use]
    pub fn oldest_event_id(&self) -> Option<&EventId> {
        self.event_history.front().map(|event| &event.event_id)
    }

    /// Removes a call only after it reached `Ended` or `Failed`.
    pub fn remove_terminal(&mut self, id: &CallId) -> Result<CallSnapshot, ApiError> {
        let snapshot = self.snapshot(id)?;
        if !matches!(snapshot.state, CallState::Ended | CallState::Failed) {
            return Err(ApiError::InvalidCommand {
                state: snapshot.state,
                command: CallCommand::End,
            });
        }
        self.calls.remove(id);
        Ok(snapshot)
    }

    /// Reclaims one terminal call after verifying hangup permission.
    pub fn remove_terminal_authorized(
        &mut self,
        principal: &AuthenticatedPrincipal,
        id: &CallId,
    ) -> Result<CallSnapshot, ApiError> {
        if let Err(error) = principal.authorize(CallCommand::End) {
            self.record_audit(
                principal,
                Some(id),
                AuditOperation::ReclaimTerminal,
                AuditOutcome::Rejected(error.code()),
            );
            return Err(error);
        }
        let result = self.remove_terminal(id);
        self.record_audit(
            principal,
            Some(id),
            AuditOperation::ReclaimTerminal,
            result.as_ref().map_or_else(
                |error| AuditOutcome::Rejected(error.code()),
                |_| AuditOutcome::Succeeded,
            ),
        );
        result
    }

    /// Records a control-plane operation performed by an integrating engine.
    ///
    /// The record is bounded by `max_pending_events`; the oldest record is
    /// evicted when the bound is reached so audit traffic cannot block call
    /// handling or grow memory without limit. The principal contains only a
    /// verified, non-secret identifier.
    pub fn record_audit(
        &mut self,
        principal: &AuthenticatedPrincipal,
        call_id: Option<&CallId>,
        operation: AuditOperation,
        outcome: AuditOutcome,
    ) {
        if self.audit_records.len() >= self.config.max_pending_events {
            self.audit_records.pop_front();
        }
        let sequence = self.next_audit_sequence;
        self.next_audit_sequence = self.next_audit_sequence.saturating_add(1);
        self.audit_records.push_back(AuditRecord {
            sequence,
            principal_id: principal.id.clone(),
            call_id: call_id.cloned(),
            operation,
            outcome,
        });
    }

    fn allocate_call_id(&mut self) -> Result<CallId, ApiError> {
        loop {
            let sequence = self.next_call_sequence;
            let next = sequence
                .checked_add(1)
                .ok_or(ApiError::IdentifierExhausted)?;
            self.next_call_sequence = next;
            let id = CallId::from_sequence(sequence);
            if !self.calls.contains_key(&id) {
                return Ok(id);
            }
        }
    }

    fn reserve_event_id(&self) -> Result<EventId, ApiError> {
        if self.events.len() >= self.config.max_pending_events {
            return Err(ApiError::EventQueueFull);
        }
        self.next_event_sequence
            .checked_add(1)
            .ok_or(ApiError::IdentifierExhausted)?;
        Ok(EventId::from_sequence(self.next_event_sequence))
    }

    fn commit_event(
        &mut self,
        event_id: EventId,
        call_id: CallId,
        kind: CallEventKind,
    ) -> LifecycleEvent {
        self.next_event_sequence += 1;
        self.lifecycle_events_total = self.lifecycle_events_total.saturating_add(1);
        let event = LifecycleEvent {
            event_id,
            call_id,
            kind,
        };
        self.events.push_back(event.clone());
        self.event_history.push_back(event.clone());
        while self.event_history.len() > self.config.max_pending_events {
            self.event_history.pop_front();
        }
        event
    }
}

fn command_transition(
    _state: CallState,
    command: CallCommand,
) -> (CallState, Option<CallEventKind>) {
    match command {
        CallCommand::InviteReceived => (CallState::Inviting, Some(CallEventKind::InviteReceived)),
        CallCommand::EarlyMedia => (CallState::Early, Some(CallEventKind::EarlyMedia)),
        CallCommand::Ringing => (CallState::Ringing, Some(CallEventKind::Ringing)),
        CallCommand::Answer => (CallState::Answered, Some(CallEventKind::Answered)),
        CallCommand::MediaStarted => (CallState::Active, Some(CallEventKind::MediaStarted)),
        CallCommand::BeginTransfer => (CallState::Transferring, Some(CallEventKind::Transferring)),
        CallCommand::CompleteTransfer => (CallState::Active, Some(CallEventKind::Transferred)),
        CallCommand::Hangup => (CallState::Ending, Some(CallEventKind::Hangup)),
        CallCommand::End => (CallState::Ended, Some(CallEventKind::Ended)),
        CallCommand::Fail => (CallState::Failed, Some(CallEventKind::Failed)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sip_dialog::{Dialog, DialogConfig};
    use sip_types::{Headers, SipMethod, SipRequest};

    fn invite_request() -> SipRequest {
        let mut headers = Headers::new();
        headers.push("Call-ID", "call-123@example.com");
        headers.push("From", "Alice <sip:alice@example.com>;tag=remote-1");
        headers.push("To", "Bob <sip:bob@example.com>");
        headers.push("CSeq", "42 INVITE");
        SipRequest {
            method: SipMethod::Invite,
            request_uri: "sip:bob@example.com".to_owned(),
            version: "SIP/2.0".to_owned(),
            headers,
            body: Vec::new(),
        }
    }

    fn principal(permissions: &[ControlPermission]) -> AuthenticatedPrincipal {
        AuthenticatedPrincipal::from_verified_claims("voice-app", permissions.iter().copied())
            .unwrap()
    }

    #[test]
    fn verified_principal_is_bounded_and_does_not_retain_credentials() {
        let principal = AuthenticatedPrincipal::from_verified_claims(
            "voice-app",
            [ControlPermission::ReadCalls, ControlPermission::ReadCalls],
        )
        .unwrap();
        assert_eq!(principal.id(), "voice-app");
        assert!(principal.has_permission(ControlPermission::ReadCalls));
        assert!(!principal.has_permission(ControlPermission::Admin));
        assert!(!format!("{principal:?}").contains("password"));

        assert_eq!(
            AuthenticatedPrincipal::from_verified_claims("", []),
            Err(ApiError::InvalidPrincipal)
        );
        assert_eq!(
            AuthenticatedPrincipal::from_verified_claims("has whitespace", []),
            Err(ApiError::InvalidPrincipal)
        );
        assert_eq!(
            AuthenticatedPrincipal::from_verified_claims("x".repeat(129), []),
            Err(ApiError::InvalidPrincipal)
        );
        assert_eq!(ControlPermission::HangupCalls.code(), "calls:hangup");
        assert_eq!(ControlPermission::Admin.code(), "calls:admin");
    }

    #[test]
    fn authorization_precedes_call_lookup_and_state_mutation() {
        let mut registry = CallRegistry::new(CallRegistryConfig::default()).unwrap();
        let id = registry.create().unwrap();
        registry.drain_events(8).unwrap();
        let read_only = principal(&[ControlPermission::ReadCalls]);
        let no_access = principal(&[]);

        assert_eq!(
            registry.apply_authorized(&read_only, &id, CallCommand::Hangup),
            Err(ApiError::PermissionDenied {
                command: CallCommand::Hangup,
                permission: ControlPermission::HangupCalls,
            })
        );
        assert_eq!(
            registry.apply_authorized(&read_only, &CallId::from_sequence(99), CallCommand::Hangup),
            Err(ApiError::PermissionDenied {
                command: CallCommand::Hangup,
                permission: ControlPermission::HangupCalls,
            })
        );
        assert_eq!(registry.snapshot(&id).unwrap().state, CallState::Created);
        assert_eq!(registry.pending_events(), 0);
        assert_eq!(
            registry.snapshot_authorized(&no_access, &id),
            Err(ApiError::ReadPermissionDenied)
        );
        assert_eq!(
            registry.snapshot_authorized(&no_access, &CallId::from_sequence(99)),
            Err(ApiError::ReadPermissionDenied)
        );
        assert_eq!(
            registry.list_authorized(&no_access, 1),
            Err(ApiError::ReadPermissionDenied)
        );
        assert_eq!(
            registry.replay_events_after_authorized(&no_access, None, 8),
            Err(ApiError::ReadPermissionDenied)
        );
        assert_eq!(
            registry.snapshot_authorized(&read_only, &id).unwrap(),
            registry.snapshot(&id).unwrap()
        );
        assert_eq!(registry.list_authorized(&read_only, 1).unwrap().len(), 1);
        assert_eq!(
            registry
                .replay_events_after_authorized(&read_only, None, 8)
                .unwrap(),
            registry.replay_events_after(None, 8).unwrap()
        );
    }

    #[test]
    fn authorized_idempotent_retries_are_replayable_but_unauthorized_retries_are_not() {
        let mut registry = CallRegistry::new(CallRegistryConfig::default()).unwrap();
        let id = registry.create().unwrap();
        registry.drain_events(8).unwrap();
        let originate = principal(&[ControlPermission::OriginateCalls]);
        let read_only = principal(&[ControlPermission::ReadCalls]);
        let command_id = CommandId::from_sequence(7);

        let first = registry
            .apply_idempotent_authorized(
                &originate,
                &id,
                CallCommand::InviteReceived,
                command_id.clone(),
            )
            .unwrap();
        assert!(!first.replayed);
        assert_eq!(registry.pending_events(), 1);
        assert_eq!(
            registry.apply_idempotent_authorized(
                &read_only,
                &id,
                CallCommand::InviteReceived,
                command_id.clone(),
            ),
            Err(ApiError::PermissionDenied {
                command: CallCommand::InviteReceived,
                permission: ControlPermission::OriginateCalls,
            })
        );
        let retry = registry
            .apply_idempotent_authorized(&originate, &id, CallCommand::InviteReceived, command_id)
            .unwrap();
        assert!(retry.replayed);
        assert_eq!(retry.event, first.event);
        assert_eq!(registry.pending_events(), 1);
        assert_eq!(registry.snapshot(&id).unwrap().state, CallState::Inviting);
    }

    #[test]
    fn authorized_operations_emit_bounded_credential_free_audit_records() {
        let mut registry = CallRegistry::new(CallRegistryConfig {
            max_calls: 1,
            max_pending_events: 2,
            max_command_keys: 2,
        })
        .unwrap();
        let id = registry.create().unwrap();
        registry.drain_events(2).unwrap();
        let originate = principal(&[ControlPermission::OriginateCalls]);
        let read_only = principal(&[ControlPermission::ReadCalls]);
        let command_id = CommandId::from_sequence(1);

        registry
            .apply_idempotent_authorized(
                &originate,
                &id,
                CallCommand::InviteReceived,
                command_id.clone(),
            )
            .unwrap();
        let retry = registry
            .apply_idempotent_authorized(&originate, &id, CallCommand::InviteReceived, command_id)
            .unwrap();
        assert!(retry.replayed);
        assert_eq!(registry.metrics().pending_audit_records, 2);

        assert_eq!(
            registry.apply_authorized(&read_only, &id, CallCommand::Hangup),
            Err(ApiError::PermissionDenied {
                command: CallCommand::Hangup,
                permission: ControlPermission::HangupCalls,
            })
        );
        let records = registry.drain_audit_records(8).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].sequence + 1, records[1].sequence);
        assert_eq!(records[0].principal_id, "voice-app");
        assert_eq!(records[0].call_id, Some(id.clone()));
        assert_eq!(
            records[0].operation,
            AuditOperation::ApplyCommand {
                command: CallCommand::InviteReceived,
                idempotent: true,
            }
        );
        assert_eq!(records[0].outcome, AuditOutcome::Replayed);
        assert_eq!(
            records[1].outcome,
            AuditOutcome::Rejected("permission_denied")
        );
        assert!(!format!("{records:?}").contains("password"));
        assert_eq!(registry.drain_audit_records(0), Err(ApiError::InvalidLimit));
    }

    #[test]
    fn authorized_terminal_reclamation_is_audited_after_resource_removal() {
        let mut registry = CallRegistry::new(CallRegistryConfig::default()).unwrap();
        let id = registry.create().unwrap();
        registry.drain_events(8).unwrap();
        let admin = principal(&[ControlPermission::Admin]);
        registry.apply(&id, CallCommand::InviteReceived).unwrap();
        registry.apply(&id, CallCommand::Hangup).unwrap();
        registry.apply(&id, CallCommand::End).unwrap();

        registry.remove_terminal_authorized(&admin, &id).unwrap();
        let record = registry.drain_audit_records(1).unwrap().pop().unwrap();
        assert_eq!(record.call_id, Some(id));
        assert_eq!(record.operation, AuditOperation::ReclaimTerminal);
        assert_eq!(record.outcome, AuditOutcome::Succeeded);
        assert_eq!(registry.pending_audit_records(), 0);
    }

    #[test]
    fn admin_permission_covers_commands_and_terminal_reclamation() {
        let mut registry = CallRegistry::new(CallRegistryConfig::default()).unwrap();
        let id = registry.create().unwrap();
        registry.drain_events(8).unwrap();
        let admin = principal(&[ControlPermission::Admin]);
        registry
            .apply_authorized(&admin, &id, CallCommand::InviteReceived)
            .unwrap();
        registry
            .apply_authorized(&admin, &id, CallCommand::Hangup)
            .unwrap();
        registry
            .apply_authorized(&admin, &id, CallCommand::End)
            .unwrap();
        assert_eq!(
            registry
                .remove_terminal_authorized(&admin, &id)
                .unwrap()
                .state,
            CallState::Ended
        );
    }

    #[test]
    fn bounded_registry_emits_stable_events_and_reclaims_terminal_calls() {
        let mut registry = CallRegistry::new(CallRegistryConfig {
            max_calls: 1,
            max_pending_events: 8,
            max_command_keys: 8,
        })
        .unwrap();
        let id = registry.create().unwrap();
        assert_eq!(id.as_str(), "call_1");
        assert_eq!(registry.pending_events(), 1);
        assert_eq!(
            registry
                .apply(&id, CallCommand::InviteReceived)
                .unwrap()
                .unwrap()
                .kind,
            CallEventKind::InviteReceived
        );
        registry.apply(&id, CallCommand::Hangup).unwrap();
        registry.apply(&id, CallCommand::End).unwrap();
        assert_eq!(registry.snapshot(&id).unwrap().state, CallState::Ended);
        registry.remove_terminal(&id).unwrap();
        assert!(matches!(registry.create(), Ok(next) if next.as_str() == "call_2"));
    }

    #[test]
    fn metrics_track_lifecycle_counters_and_bounded_gauges() {
        let mut registry = CallRegistry::new(CallRegistryConfig {
            max_calls: 1,
            max_pending_events: 8,
            max_command_keys: 1,
        })
        .unwrap();
        assert_eq!(registry.metrics(), CallMetrics::default());

        let id = registry.create().unwrap();
        let created = registry.metrics();
        assert_eq!(created.calls_started_total, 1);
        assert_eq!(created.calls_active, 1);
        assert_eq!(created.calls_retained, 1);
        assert_eq!(created.lifecycle_events_total, 1);
        assert_eq!(created.pending_events, 1);
        assert_eq!(created.retained_event_history, 1);
        assert_eq!(created.retained_command_keys, 0);
        registry.drain_events(8).unwrap();

        registry.apply(&id, CallCommand::InviteReceived).unwrap();
        registry.apply(&id, CallCommand::Ringing).unwrap();
        registry.apply(&id, CallCommand::Answer).unwrap();
        let answered = registry.metrics();
        assert_eq!(answered.calls_started_total, 1);
        assert_eq!(answered.calls_answered_total, 1);
        assert_eq!(answered.calls_failed_total, 0);
        assert_eq!(answered.calls_completed_total, 0);
        assert_eq!(answered.calls_active, 1);
        assert_eq!(answered.lifecycle_events_total, 4);
        assert_eq!(answered.pending_events, 3);
        assert_eq!(answered.retained_event_history, 4);

        registry.apply(&id, CallCommand::Hangup).unwrap();
        registry.apply(&id, CallCommand::End).unwrap();
        let ended = registry.metrics();
        assert_eq!(ended.calls_completed_total, 1);
        assert_eq!(ended.calls_active, 0);
        assert_eq!(ended.lifecycle_events_total, 6);
        registry.remove_terminal(&id).unwrap();
        assert_eq!(registry.metrics().calls_started_total, 1);
        assert_eq!(registry.metrics().calls_active, 0);
        assert_eq!(registry.metrics().calls_retained, 0);
    }

    #[test]
    fn failed_calls_are_counted_once_and_idempotency_gauge_is_bounded() {
        let mut registry = CallRegistry::new(CallRegistryConfig {
            max_calls: 1,
            max_pending_events: 8,
            max_command_keys: 1,
        })
        .unwrap();
        let id = registry.create().unwrap();
        registry.drain_events(8).unwrap();
        let command_id = CommandId::from_sequence(1);
        registry
            .apply_idempotent(&id, CallCommand::Fail, command_id.clone())
            .unwrap();
        let failed = registry.metrics();
        assert_eq!(failed.calls_failed_total, 1);
        assert_eq!(failed.calls_active, 0);
        assert_eq!(failed.retained_command_keys, 1);
        let replay = registry
            .apply_idempotent(&id, CallCommand::Fail, command_id)
            .unwrap();
        assert!(replay.replayed);
        assert_eq!(registry.metrics().calls_failed_total, 1);
        assert_eq!(registry.metrics().retained_command_keys, 1);
    }

    #[test]
    fn invalid_commands_do_not_mutate_state_and_error_codes_are_stable() {
        let mut registry = CallRegistry::new(CallRegistryConfig::default()).unwrap();
        let id = registry.create().unwrap();
        let error = registry.apply(&id, CallCommand::MediaStarted).unwrap_err();
        assert_eq!(error.code(), "invalid_command");
        assert_eq!(registry.snapshot(&id).unwrap().state, CallState::Created);
        assert_eq!(registry.drain_events(0), Err(ApiError::InvalidLimit));
        assert_eq!(registry.list(0), Err(ApiError::InvalidLimit));
    }

    #[test]
    fn dialog_binding_is_visible_without_using_sip_call_id_as_app_id() {
        let mut registry = CallRegistry::new(CallRegistryConfig::default()).unwrap();
        let id = registry.create().unwrap();
        let dialog =
            Dialog::from_uas_invite(&invite_request(), "local-1", DialogConfig::default()).unwrap();
        registry.bind_dialog(&id, dialog.id()).unwrap();
        let snapshot = registry.snapshot(&id).unwrap();
        assert_eq!(snapshot.id.as_str(), "call_1");
        assert_eq!(
            snapshot.dialog_id.unwrap().call_id(),
            "call-123@example.com"
        );
        assert_eq!(
            registry.bind_dialog(&id, dialog.id()),
            Err(ApiError::DialogAlreadyBound)
        );
    }

    #[test]
    fn limits_are_enforced_before_state_or_event_mutation() {
        let mut registry = CallRegistry::new(CallRegistryConfig {
            max_calls: 2,
            max_pending_events: 1,
            max_command_keys: 1,
        })
        .unwrap();
        let first = registry.create().unwrap();
        assert_eq!(
            registry.apply(&first, CallCommand::InviteReceived),
            Err(ApiError::EventQueueFull)
        );
        assert_eq!(registry.snapshot(&first).unwrap().state, CallState::Created);
        assert_eq!(registry.create(), Err(ApiError::EventQueueFull));
        registry.drain_events(1).unwrap();
        assert_eq!(registry.create().unwrap().as_str(), "call_2");
    }

    #[test]
    fn terminal_end_emits_an_explicit_event_and_replay_is_cursored() {
        let mut registry = CallRegistry::new(CallRegistryConfig {
            max_calls: 1,
            max_pending_events: 8,
            max_command_keys: 8,
        })
        .unwrap();
        let id = registry.create().unwrap();
        registry.apply(&id, CallCommand::InviteReceived).unwrap();
        registry.apply(&id, CallCommand::Hangup).unwrap();
        registry.apply(&id, CallCommand::End).unwrap();

        let events = registry.drain_events(8).unwrap();
        assert_eq!(
            events.iter().map(|event| event.kind).collect::<Vec<_>>(),
            vec![
                CallEventKind::Created,
                CallEventKind::InviteReceived,
                CallEventKind::Hangup,
                CallEventKind::Ended,
            ]
        );
        assert_eq!(registry.latest_event_id(), Some(&events[3].event_id));
        assert_eq!(registry.oldest_event_id(), Some(&events[0].event_id));
        assert_eq!(
            registry
                .replay_events_after(Some(&events[0].event_id), 2)
                .unwrap(),
            events[1..3].to_vec()
        );
        assert_eq!(
            registry
                .replay_events_after(Some(&events[3].event_id), 2)
                .unwrap(),
            Vec::<LifecycleEvent>::new()
        );
    }

    #[test]
    fn replay_rejects_evicted_cursors_instead_of_silently_skipping_events() {
        let mut registry = CallRegistry::new(CallRegistryConfig {
            max_calls: 1,
            max_pending_events: 2,
            max_command_keys: 2,
        })
        .unwrap();
        let id = registry.create().unwrap();
        let created = registry.drain_events(2).unwrap().pop().unwrap();
        registry.apply(&id, CallCommand::InviteReceived).unwrap();
        registry.drain_events(2).unwrap();
        registry.apply(&id, CallCommand::Ringing).unwrap();
        registry.drain_events(2).unwrap();

        assert_eq!(
            registry.replay_events_after(Some(&created.event_id), 1),
            Err(ApiError::EventHistoryUnavailable)
        );
        assert_eq!(registry.replay_events_after(None, 8).unwrap().len(), 2);
        assert_eq!(
            registry.replay_events_after(None, 0),
            Err(ApiError::InvalidLimit)
        );
    }

    #[test]
    fn idempotent_command_retries_replay_once_without_duplicate_events() {
        let mut registry = CallRegistry::new(CallRegistryConfig {
            max_calls: 1,
            max_pending_events: 8,
            max_command_keys: 4,
        })
        .unwrap();
        let id = registry.create().unwrap();
        registry.drain_events(8).unwrap();
        let command_id = CommandId::from_sequence(1);

        let first = registry
            .apply_idempotent(&id, CallCommand::InviteReceived, command_id.clone())
            .unwrap();
        assert!(!first.replayed);
        let first_event = first.event.clone().unwrap();
        assert_eq!(registry.drain_events(8).unwrap(), vec![first_event.clone()]);

        let retry = registry
            .apply_idempotent(&id, CallCommand::InviteReceived, command_id.clone())
            .unwrap();
        assert!(retry.replayed);
        assert_eq!(retry.event, Some(first_event));
        assert_eq!(registry.pending_events(), 0);
        assert_eq!(registry.snapshot(&id).unwrap().state, CallState::Inviting);

        assert_eq!(
            registry.apply_idempotent(&id, CallCommand::Ringing, command_id),
            Err(ApiError::IdempotencyConflict)
        );
        assert_eq!(registry.pending_events(), 0);
        assert_eq!(registry.snapshot(&id).unwrap().state, CallState::Inviting);
    }

    #[test]
    fn idempotency_keys_are_bounded_and_evicted_in_emission_order() {
        let mut registry = CallRegistry::new(CallRegistryConfig {
            max_calls: 1,
            max_pending_events: 8,
            max_command_keys: 1,
        })
        .unwrap();
        let id = registry.create().unwrap();
        registry.drain_events(8).unwrap();
        let first_key = CommandId::from_sequence(1);
        let second_key = CommandId::from_sequence(2);
        registry
            .apply_idempotent(&id, CallCommand::InviteReceived, first_key.clone())
            .unwrap();
        registry.drain_events(8).unwrap();
        registry
            .apply_idempotent(&id, CallCommand::Ringing, second_key)
            .unwrap();
        registry.drain_events(8).unwrap();

        let reused = registry
            .apply_idempotent(&id, CallCommand::Answer, first_key)
            .unwrap();
        assert!(!reused.replayed);
        assert_eq!(reused.event.unwrap().kind, CallEventKind::Answered);
        assert_eq!(registry.snapshot(&id).unwrap().state, CallState::Answered);
    }

    #[test]
    fn audio_negotiation_retains_codec_direction_and_remote_endpoint() {
        let mut registry = CallRegistry::new(CallRegistryConfig::default()).unwrap();
        let id = registry.create().unwrap();
        let local =
            SessionDescription::new_audio("- 2 2 IN IP4 192.0.2.20", "IN IP4 192.0.2.20", 5000);
        let remote = sdp::parse(
            b"v=0\r\no=- 1 1 IN IP4 192.0.2.10\r\ns=-\r\nc=IN IP4 192.0.2.10\r\nt=0 0\r\na=sendonly\r\nm=audio 4000 RTP/AVP 96\r\na=rtpmap:96 PCMU/8000\r\n",
        )
        .unwrap();

        let negotiated = registry.negotiate_audio(&id, &local, &remote).unwrap();
        assert_eq!(negotiated.local_codec.payload_type, 0);
        assert_eq!(negotiated.remote_codec.payload_type, 96);
        assert_eq!(negotiated.direction, Direction::RecvOnly);
        assert_eq!(
            negotiated.remote_connection.as_deref(),
            Some("IN IP4 192.0.2.10")
        );
        assert_eq!(negotiated.remote_port, 4000);
        assert_eq!(registry.media(&id).unwrap(), Some(negotiated.clone()));
        assert_eq!(registry.snapshot(&id).unwrap().media, Some(negotiated));
    }

    #[test]
    fn failed_audio_negotiation_does_not_replace_existing_binding() {
        let mut registry = CallRegistry::new(CallRegistryConfig::default()).unwrap();
        let id = registry.create().unwrap();
        let local =
            SessionDescription::new_audio("- 2 2 IN IP4 192.0.2.20", "IN IP4 192.0.2.20", 5000);
        let good = sdp::parse(
            b"v=0\r\no=- 1 1 IN IP4 192.0.2.10\r\ns=-\r\nc=IN IP4 192.0.2.10\r\nt=0 0\r\na=sendonly\r\nm=audio 4000 RTP/AVP 96\r\na=rtpmap:96 PCMU/8000\r\n",
        )
        .unwrap();
        let expected = registry.negotiate_audio(&id, &local, &good).unwrap();
        let remote = sdp::parse(
            b"v=0\r\no=- 1 1 IN IP4 192.0.2.10\r\ns=-\r\nc=IN IP4 192.0.2.10\r\nt=0 0\r\nm=audio 4000 RTP/AVP 9\r\na=rtpmap:9 G722/8000\r\n",
        )
        .unwrap();
        assert_eq!(
            registry.negotiate_audio(&id, &local, &remote),
            Err(ApiError::NoCommonCodec)
        );
        assert_eq!(registry.media(&id).unwrap(), Some(expected.clone()));

        let rejected = sdp::parse(
            b"v=0\r\no=- 1 1 IN IP4 192.0.2.10\r\ns=-\r\nt=0 0\r\nm=audio 0 RTP/AVP 0\r\n",
        )
        .unwrap();
        assert_eq!(
            registry.negotiate_audio(&id, &local, &rejected),
            Err(ApiError::MediaRejected)
        );
        assert_eq!(registry.media(&id).unwrap(), Some(expected));
    }
}
