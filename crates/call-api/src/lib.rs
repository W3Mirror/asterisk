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
    next_call_sequence: u64,
    next_event_sequence: u64,
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
            next_call_sequence: 1,
            next_event_sequence: 1,
        })
    }

    /// Returns the configured resource bounds.
    #[must_use]
    pub fn config(&self) -> CallRegistryConfig {
        self.config
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
        Ok(match event_kind {
            Some(kind) => Some(self.commit_event(
                event_id.ok_or(ApiError::IdentifierExhausted)?,
                id.clone(),
                kind,
            )),
            None => None,
        })
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

    /// Returns the number of events waiting to be delivered.
    #[must_use]
    pub fn pending_events(&self) -> usize {
        self.events.len()
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
