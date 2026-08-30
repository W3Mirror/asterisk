//! Bounded provider-neutral call-leg bridge state.
//!
//! A bridge retains one inbound caller leg and its AI stream while a human
//! second leg is dialed. Human-leg failure deterministically resumes AI media,
//! so escalation does not require replacing the inbound call. This crate owns
//! no sockets, media buffers, provider policy, or async runtime.

use std::{
    collections::{HashMap, VecDeque},
    error::Error,
    fmt::{Display, Formatter},
};

use call_core::{BridgeId, CallId, EventId, LegId, StreamId};

const DEFAULT_MAX_BRIDGES: usize = 4_096;
const DEFAULT_MAX_PENDING_EVENTS: usize = 16_384;

/// Resource limits for a [`BridgeRegistry`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BridgeRegistryConfig {
    /// Maximum retained bridge records, including terminal records awaiting reclamation.
    pub max_bridges: usize,
    /// Maximum events retained until the control plane drains them.
    pub max_pending_events: usize,
}

impl Default for BridgeRegistryConfig {
    fn default() -> Self {
        Self {
            max_bridges: DEFAULT_MAX_BRIDGES,
            max_pending_events: DEFAULT_MAX_PENDING_EVENTS,
        }
    }
}

impl BridgeRegistryConfig {
    fn validate(self) -> Result<Self, BridgeError> {
        if self.max_bridges == 0 || self.max_pending_events == 0 {
            return Err(BridgeError::InvalidConfig);
        }
        Ok(self)
    }
}

/// High-level routing state for one caller bridge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BridgeState {
    /// Caller media is connected to the retained AI stream.
    AiActive,
    /// AI remains active while an outbound human leg is being established.
    ConnectingHuman,
    /// Caller media is connected to the established human leg.
    HumanActive,
    /// Bridge forwarding stopped and the record may be reclaimed.
    Ended,
}

/// Human SIP/PSTN call and media-leg identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HumanLeg {
    /// Outbound application call identity.
    pub call_id: CallId,
    /// Media/signaling leg identity owned by that call.
    pub leg_id: LegId,
}

/// Stable read-only bridge state exposed to the control plane.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BridgeSnapshot {
    /// Stable bridge identity.
    pub id: BridgeId,
    /// Original inbound call, retained across destination switches.
    pub caller_call_id: CallId,
    /// Original inbound media/signaling leg.
    pub caller_leg_id: LegId,
    /// AI media stream retained for deterministic fail-back.
    pub ai_stream_id: StreamId,
    /// Current routing state.
    pub state: BridgeState,
    /// Established human destination when it is active.
    pub active_human: Option<HumanLeg>,
    /// Outbound human destination while it is connecting.
    pub pending_human: Option<HumanLeg>,
}

/// Operation used in stable invalid-state diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BridgeOperation {
    /// Start establishing a human second leg.
    BeginHuman,
    /// Activate the established human second leg.
    CompleteHuman,
    /// Report failure of a pending or active human leg.
    FailHuman,
    /// Switch an active human bridge back to AI.
    ResumeAi,
    /// Stop all forwarding and enter terminal state.
    End,
    /// Remove an ended bridge record and release its owned endpoints.
    Reclaim,
}

/// Observable bridge lifecycle event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BridgeEventKind {
    /// An AI-backed caller bridge was created.
    Created,
    /// A human second-leg attempt started while AI remained active.
    HumanConnecting,
    /// The human second leg became the active destination.
    HumanConnected,
    /// The human leg failed and routing fell back to AI.
    HumanFailed,
    /// The control plane explicitly restored AI routing.
    AiResumed,
    /// Bridge forwarding ended.
    Ended,
}

/// Ordered event emitted by one bridge transition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BridgeEvent {
    /// Stable event identity.
    pub event_id: EventId,
    /// Bridge that emitted the event.
    pub bridge_id: BridgeId,
    /// Lifecycle event kind.
    pub kind: BridgeEventKind,
}

/// Errors returned by bounded bridge operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BridgeError {
    /// At least one configured bound was zero.
    InvalidConfig,
    /// The bridge registry reached its configured record limit.
    BridgeLimitReached,
    /// A call, leg, or AI stream is already owned by a retained bridge.
    EndpointInUse,
    /// The requested bridge does not exist.
    UnknownBridge,
    /// The event queue reached its configured limit.
    EventQueueFull,
    /// A list or drain limit was zero.
    InvalidLimit,
    /// The operation is invalid in the bridge's current state.
    InvalidOperation {
        /// Current bridge state.
        state: BridgeState,
        /// Rejected operation.
        operation: BridgeOperation,
    },
    /// A generated bridge or event sequence cannot advance safely.
    IdentifierExhausted,
}

impl Display for BridgeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfig => formatter.write_str("bridge registry bounds must be non-zero"),
            Self::BridgeLimitReached => formatter.write_str("bridge registry reached its limit"),
            Self::EndpointInUse => {
                formatter.write_str("call, leg, or AI stream is already assigned to a bridge")
            }
            Self::UnknownBridge => formatter.write_str("bridge is not registered"),
            Self::EventQueueFull => formatter.write_str("bridge event queue reached its limit"),
            Self::InvalidLimit => formatter.write_str("bridge API limit must be non-zero"),
            Self::InvalidOperation { state, operation } => {
                write!(
                    formatter,
                    "operation {operation:?} is invalid in state {state:?}"
                )
            }
            Self::IdentifierExhausted => {
                formatter.write_str("bridge or event identifier sequence exhausted")
            }
        }
    }
}

impl Error for BridgeError {}

#[derive(Clone, Debug)]
struct BridgeEntry {
    caller_call_id: CallId,
    caller_leg_id: LegId,
    ai_stream_id: StreamId,
    state: BridgeState,
    active_human: Option<HumanLeg>,
    pending_human: Option<HumanLeg>,
}

/// Bounded bridge registry with explicit transitions and deterministic events.
#[derive(Clone, Debug)]
pub struct BridgeRegistry {
    config: BridgeRegistryConfig,
    bridges: HashMap<BridgeId, BridgeEntry>,
    events: VecDeque<BridgeEvent>,
    next_bridge_sequence: u64,
    next_event_sequence: u64,
}

impl BridgeRegistry {
    /// Creates an empty bridge registry.
    ///
    /// # Errors
    ///
    /// Returns an error when any configured bound is zero.
    pub fn new(config: BridgeRegistryConfig) -> Result<Self, BridgeError> {
        Ok(Self {
            config: config.validate()?,
            bridges: HashMap::new(),
            events: VecDeque::new(),
            next_bridge_sequence: 1,
            next_event_sequence: 1,
        })
    }

    /// Creates an AI-backed bridge for one stable inbound caller leg.
    ///
    /// # Errors
    ///
    /// Returns an error for exhausted bounds, identifiers, or endpoint ownership.
    pub fn create_ai(
        &mut self,
        caller_call_id: CallId,
        caller_leg_id: LegId,
        ai_stream_id: StreamId,
    ) -> Result<(BridgeId, BridgeEvent), BridgeError> {
        if self.bridges.len() >= self.config.max_bridges {
            return Err(BridgeError::BridgeLimitReached);
        }
        if self.call_in_use(&caller_call_id)
            || self.leg_in_use(&caller_leg_id)
            || self.stream_in_use(&ai_stream_id)
        {
            return Err(BridgeError::EndpointInUse);
        }
        let event_id = self.reserve_event_id()?;
        let bridge_id = self.allocate_bridge_id()?;
        self.bridges.insert(
            bridge_id.clone(),
            BridgeEntry {
                caller_call_id,
                caller_leg_id,
                ai_stream_id,
                state: BridgeState::AiActive,
                active_human: None,
                pending_human: None,
            },
        );
        let event = self.commit_event(event_id, bridge_id.clone(), BridgeEventKind::Created);
        Ok((bridge_id, event))
    }

    /// Starts a human second leg while preserving AI routing until completion.
    ///
    /// # Errors
    ///
    /// Returns an error unless the bridge is AI-active and both human identities
    /// are unowned. Rejection leaves bridge and event state unchanged.
    pub fn begin_human(
        &mut self,
        id: &BridgeId,
        call_id: CallId,
        leg_id: LegId,
    ) -> Result<BridgeEvent, BridgeError> {
        self.require_state(id, BridgeState::AiActive, BridgeOperation::BeginHuman)?;
        if self.call_in_use(&call_id) || self.leg_in_use(&leg_id) {
            return Err(BridgeError::EndpointInUse);
        }
        let event_id = self.reserve_event_id()?;
        let entry = self.bridges.get_mut(id).ok_or(BridgeError::UnknownBridge)?;
        entry.pending_human = Some(HumanLeg { call_id, leg_id });
        entry.state = BridgeState::ConnectingHuman;
        Ok(self.commit_event(event_id, id.clone(), BridgeEventKind::HumanConnecting))
    }

    /// Activates the pending human leg and switches caller media away from AI.
    ///
    /// # Errors
    ///
    /// Returns an error unless the bridge is connecting a human leg.
    pub fn complete_human(&mut self, id: &BridgeId) -> Result<BridgeEvent, BridgeError> {
        self.require_state(
            id,
            BridgeState::ConnectingHuman,
            BridgeOperation::CompleteHuman,
        )?;
        let event_id = self.reserve_event_id()?;
        let entry = self.bridges.get_mut(id).ok_or(BridgeError::UnknownBridge)?;
        entry.active_human = entry.pending_human.take();
        entry.state = BridgeState::HumanActive;
        Ok(self.commit_event(event_id, id.clone(), BridgeEventKind::HumanConnected))
    }

    /// Fails a pending or active human leg and deterministically restores AI.
    ///
    /// # Errors
    ///
    /// Returns an error unless a human leg is connecting or active.
    pub fn fail_human(&mut self, id: &BridgeId) -> Result<BridgeEvent, BridgeError> {
        let state = self.state(id)?;
        if !matches!(
            state,
            BridgeState::ConnectingHuman | BridgeState::HumanActive
        ) {
            return Err(BridgeError::InvalidOperation {
                state,
                operation: BridgeOperation::FailHuman,
            });
        }
        let event_id = self.reserve_event_id()?;
        let entry = self.bridges.get_mut(id).ok_or(BridgeError::UnknownBridge)?;
        entry.pending_human = None;
        entry.active_human = None;
        entry.state = BridgeState::AiActive;
        Ok(self.commit_event(event_id, id.clone(), BridgeEventKind::HumanFailed))
    }

    /// Explicitly switches an active human bridge back to the retained AI stream.
    ///
    /// # Errors
    ///
    /// Returns an error unless the human leg is active.
    pub fn resume_ai(&mut self, id: &BridgeId) -> Result<BridgeEvent, BridgeError> {
        self.require_state(id, BridgeState::HumanActive, BridgeOperation::ResumeAi)?;
        let event_id = self.reserve_event_id()?;
        let entry = self.bridges.get_mut(id).ok_or(BridgeError::UnknownBridge)?;
        entry.active_human = None;
        entry.state = BridgeState::AiActive;
        Ok(self.commit_event(event_id, id.clone(), BridgeEventKind::AiResumed))
    }

    /// Ends forwarding and releases human endpoints from the retained snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error when the bridge is unknown or already ended.
    pub fn end(&mut self, id: &BridgeId) -> Result<BridgeEvent, BridgeError> {
        let state = self.state(id)?;
        if state == BridgeState::Ended {
            return Err(BridgeError::InvalidOperation {
                state,
                operation: BridgeOperation::End,
            });
        }
        let event_id = self.reserve_event_id()?;
        let entry = self.bridges.get_mut(id).ok_or(BridgeError::UnknownBridge)?;
        entry.pending_human = None;
        entry.active_human = None;
        entry.state = BridgeState::Ended;
        Ok(self.commit_event(event_id, id.clone(), BridgeEventKind::Ended))
    }

    /// Removes an ended bridge so its caller, leg, stream, and record slot can be reused.
    ///
    /// # Errors
    ///
    /// Returns an error unless the bridge exists and has ended.
    pub fn remove_terminal(&mut self, id: &BridgeId) -> Result<BridgeSnapshot, BridgeError> {
        let snapshot = self.snapshot(id)?;
        if snapshot.state != BridgeState::Ended {
            return Err(BridgeError::InvalidOperation {
                state: snapshot.state,
                operation: BridgeOperation::Reclaim,
            });
        }
        self.bridges.remove(id);
        Ok(snapshot)
    }

    /// Returns one stable bridge snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error when the bridge is unknown.
    pub fn snapshot(&self, id: &BridgeId) -> Result<BridgeSnapshot, BridgeError> {
        let entry = self.bridges.get(id).ok_or(BridgeError::UnknownBridge)?;
        Ok(snapshot(id, entry))
    }

    /// Lists retained bridges in deterministic identifier order.
    ///
    /// # Errors
    ///
    /// Returns an error when `limit` is zero.
    pub fn list(&self, limit: usize) -> Result<Vec<BridgeSnapshot>, BridgeError> {
        if limit == 0 {
            return Err(BridgeError::InvalidLimit);
        }
        let mut snapshots = self
            .bridges
            .iter()
            .map(|(id, entry)| snapshot(id, entry))
            .collect::<Vec<_>>();
        snapshots.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
        snapshots.truncate(limit);
        Ok(snapshots)
    }

    /// Returns the configured registry bounds.
    #[must_use]
    pub fn config(&self) -> BridgeRegistryConfig {
        self.config
    }

    /// Returns the number of undelivered bridge events.
    #[must_use]
    pub fn pending_events(&self) -> usize {
        self.events.len()
    }

    /// Drains bridge events in emission order.
    ///
    /// # Errors
    ///
    /// Returns an error when `limit` is zero.
    pub fn drain_events(&mut self, limit: usize) -> Result<Vec<BridgeEvent>, BridgeError> {
        if limit == 0 {
            return Err(BridgeError::InvalidLimit);
        }
        Ok(self.events.drain(..limit.min(self.events.len())).collect())
    }

    fn state(&self, id: &BridgeId) -> Result<BridgeState, BridgeError> {
        Ok(self
            .bridges
            .get(id)
            .ok_or(BridgeError::UnknownBridge)?
            .state)
    }

    fn require_state(
        &self,
        id: &BridgeId,
        expected: BridgeState,
        operation: BridgeOperation,
    ) -> Result<(), BridgeError> {
        let state = self.state(id)?;
        if state != expected {
            return Err(BridgeError::InvalidOperation { state, operation });
        }
        Ok(())
    }

    fn call_in_use(&self, id: &CallId) -> bool {
        self.bridges.values().any(|entry| {
            &entry.caller_call_id == id
                || entry
                    .active_human
                    .as_ref()
                    .is_some_and(|human| &human.call_id == id)
                || entry
                    .pending_human
                    .as_ref()
                    .is_some_and(|human| &human.call_id == id)
        })
    }

    fn leg_in_use(&self, id: &LegId) -> bool {
        self.bridges.values().any(|entry| {
            &entry.caller_leg_id == id
                || entry
                    .active_human
                    .as_ref()
                    .is_some_and(|human| &human.leg_id == id)
                || entry
                    .pending_human
                    .as_ref()
                    .is_some_and(|human| &human.leg_id == id)
        })
    }

    fn stream_in_use(&self, id: &StreamId) -> bool {
        self.bridges.values().any(|entry| &entry.ai_stream_id == id)
    }

    fn allocate_bridge_id(&mut self) -> Result<BridgeId, BridgeError> {
        loop {
            let sequence = self.next_bridge_sequence;
            self.next_bridge_sequence = sequence
                .checked_add(1)
                .ok_or(BridgeError::IdentifierExhausted)?;
            let id = BridgeId::from_sequence(sequence);
            if !self.bridges.contains_key(&id) {
                return Ok(id);
            }
        }
    }

    fn reserve_event_id(&self) -> Result<EventId, BridgeError> {
        if self.events.len() >= self.config.max_pending_events {
            return Err(BridgeError::EventQueueFull);
        }
        self.next_event_sequence
            .checked_add(1)
            .ok_or(BridgeError::IdentifierExhausted)?;
        Ok(EventId::from_sequence(self.next_event_sequence))
    }

    fn commit_event(
        &mut self,
        event_id: EventId,
        bridge_id: BridgeId,
        kind: BridgeEventKind,
    ) -> BridgeEvent {
        self.next_event_sequence += 1;
        let event = BridgeEvent {
            event_id,
            bridge_id,
            kind,
        };
        self.events.push_back(event.clone());
        event
    }
}

fn snapshot(id: &BridgeId, entry: &BridgeEntry) -> BridgeSnapshot {
    BridgeSnapshot {
        id: id.clone(),
        caller_call_id: entry.caller_call_id.clone(),
        caller_leg_id: entry.caller_leg_id.clone(),
        ai_stream_id: entry.ai_stream_id.clone(),
        state: entry.state,
        active_human: entry.active_human.clone(),
        pending_human: entry.pending_human.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caller(sequence: u64) -> (CallId, LegId, StreamId) {
        (
            CallId::from_sequence(sequence),
            LegId::from_sequence(sequence),
            StreamId::from_sequence(sequence),
        )
    }

    fn human(sequence: u64) -> (CallId, LegId) {
        (
            CallId::from_sequence(sequence),
            LegId::from_sequence(sequence),
        )
    }

    #[test]
    fn switches_ai_to_human_and_back_without_replacing_the_caller() {
        let mut registry = BridgeRegistry::new(BridgeRegistryConfig::default()).unwrap();
        let (caller_call, caller_leg, stream) = caller(1);
        let (id, created) = registry
            .create_ai(caller_call.clone(), caller_leg.clone(), stream.clone())
            .unwrap();
        let (human_call, human_leg) = human(2);

        assert_eq!(created.kind, BridgeEventKind::Created);
        registry
            .begin_human(&id, human_call.clone(), human_leg.clone())
            .unwrap();
        let connecting = registry.snapshot(&id).unwrap();
        assert_eq!(connecting.state, BridgeState::ConnectingHuman);
        assert_eq!(connecting.caller_call_id, caller_call);
        assert_eq!(connecting.ai_stream_id, stream);
        assert_eq!(
            connecting.pending_human,
            Some(HumanLeg {
                call_id: human_call.clone(),
                leg_id: human_leg.clone(),
            })
        );

        registry.complete_human(&id).unwrap();
        let active = registry.snapshot(&id).unwrap();
        assert_eq!(active.state, BridgeState::HumanActive);
        assert!(active.pending_human.is_none());
        assert_eq!(
            active.active_human,
            Some(HumanLeg {
                call_id: human_call,
                leg_id: human_leg,
            })
        );

        registry.resume_ai(&id).unwrap();
        let resumed = registry.snapshot(&id).unwrap();
        assert_eq!(resumed.state, BridgeState::AiActive);
        assert_eq!(resumed.caller_leg_id, caller_leg);
        assert!(resumed.active_human.is_none());
        assert_eq!(
            registry
                .drain_events(8)
                .unwrap()
                .into_iter()
                .map(|event| event.kind)
                .collect::<Vec<_>>(),
            vec![
                BridgeEventKind::Created,
                BridgeEventKind::HumanConnecting,
                BridgeEventKind::HumanConnected,
                BridgeEventKind::AiResumed,
            ]
        );
    }

    #[test]
    fn partial_and_active_human_failures_restore_ai_and_release_endpoints() {
        let mut registry = BridgeRegistry::new(BridgeRegistryConfig::default()).unwrap();
        let (caller_call, caller_leg, stream) = caller(1);
        let (id, _) = registry.create_ai(caller_call, caller_leg, stream).unwrap();
        let (human_call, human_leg) = human(2);

        registry
            .begin_human(&id, human_call.clone(), human_leg.clone())
            .unwrap();
        assert_eq!(
            registry.fail_human(&id).unwrap().kind,
            BridgeEventKind::HumanFailed
        );
        assert_eq!(registry.snapshot(&id).unwrap().state, BridgeState::AiActive);

        registry
            .begin_human(&id, human_call.clone(), human_leg.clone())
            .unwrap();
        registry.complete_human(&id).unwrap();
        registry.fail_human(&id).unwrap();
        let failed = registry.snapshot(&id).unwrap();
        assert_eq!(failed.state, BridgeState::AiActive);
        assert!(failed.active_human.is_none());

        registry.begin_human(&id, human_call, human_leg).unwrap();
    }

    #[test]
    fn event_backpressure_and_invalid_transitions_are_atomic() {
        let mut registry = BridgeRegistry::new(BridgeRegistryConfig {
            max_bridges: 2,
            max_pending_events: 1,
        })
        .unwrap();
        let (caller_call, caller_leg, stream) = caller(1);
        let (id, _) = registry.create_ai(caller_call, caller_leg, stream).unwrap();
        let before = registry.snapshot(&id).unwrap();
        let (human_call, human_leg) = human(2);

        assert_eq!(
            registry.begin_human(&id, human_call.clone(), human_leg.clone()),
            Err(BridgeError::EventQueueFull)
        );
        assert_eq!(registry.snapshot(&id).unwrap(), before);
        registry.drain_events(1).unwrap();
        assert_eq!(
            registry.complete_human(&id),
            Err(BridgeError::InvalidOperation {
                state: BridgeState::AiActive,
                operation: BridgeOperation::CompleteHuman,
            })
        );
        assert_eq!(registry.snapshot(&id).unwrap(), before);
        registry.begin_human(&id, human_call, human_leg).unwrap();
    }

    #[test]
    fn terminal_cleanup_releases_every_endpoint_and_registry_capacity() {
        let mut registry = BridgeRegistry::new(BridgeRegistryConfig {
            max_bridges: 1,
            max_pending_events: 8,
        })
        .unwrap();
        let (caller_call, caller_leg, stream) = caller(1);
        let (id, _) = registry
            .create_ai(caller_call.clone(), caller_leg.clone(), stream.clone())
            .unwrap();
        let (human_call, human_leg) = human(2);
        registry.begin_human(&id, human_call, human_leg).unwrap();

        registry.end(&id).unwrap();
        let ended = registry.snapshot(&id).unwrap();
        assert_eq!(ended.state, BridgeState::Ended);
        assert!(ended.pending_human.is_none());
        assert!(ended.active_human.is_none());
        assert_eq!(
            registry.create_ai(caller_call.clone(), caller_leg.clone(), stream.clone()),
            Err(BridgeError::BridgeLimitReached)
        );
        assert_eq!(registry.remove_terminal(&id).unwrap(), ended);

        let (next, _) = registry.create_ai(caller_call, caller_leg, stream).unwrap();
        assert_eq!(next, BridgeId::from_sequence(2));
    }

    #[test]
    fn validates_bounds_ownership_and_api_limits() {
        assert!(matches!(
            BridgeRegistry::new(BridgeRegistryConfig {
                max_bridges: 0,
                max_pending_events: 1,
            }),
            Err(BridgeError::InvalidConfig)
        ));
        let mut registry = BridgeRegistry::new(BridgeRegistryConfig::default()).unwrap();
        assert_eq!(registry.config(), BridgeRegistryConfig::default());
        let (caller_call, caller_leg, stream) = caller(1);
        let (id, _) = registry
            .create_ai(caller_call.clone(), caller_leg.clone(), stream.clone())
            .unwrap();

        assert_eq!(
            registry.create_ai(
                caller_call,
                LegId::from_sequence(9),
                StreamId::from_sequence(9)
            ),
            Err(BridgeError::EndpointInUse)
        );
        assert_eq!(
            registry.create_ai(
                CallId::from_sequence(9),
                caller_leg,
                StreamId::from_sequence(9)
            ),
            Err(BridgeError::EndpointInUse)
        );
        assert_eq!(
            registry.create_ai(CallId::from_sequence(9), LegId::from_sequence(9), stream),
            Err(BridgeError::EndpointInUse)
        );
        assert_eq!(registry.list(0), Err(BridgeError::InvalidLimit));
        assert_eq!(registry.drain_events(0), Err(BridgeError::InvalidLimit));
        assert_eq!(
            registry.remove_terminal(&id),
            Err(BridgeError::InvalidOperation {
                state: BridgeState::AiActive,
                operation: BridgeOperation::Reclaim,
            })
        );
        assert_eq!(registry.snapshot(&id).unwrap().state, BridgeState::AiActive);
    }
}
