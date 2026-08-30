//! Deterministic, offline SIP, media, and call-bridge scenario replay.
//!
//! The replay boundary consumes owned wire fixtures and explicit monotonic
//! timestamps. It owns no sockets, sleeps, wall clock, credentials, or provider
//! configuration, so synthetic scenarios and later sanitized captures can use
//! the same execution path. Call-engine, media-session, and bridge-registry
//! state commit atomically only after every scenario step succeeds.

use std::{
    error::Error,
    fmt::{Display, Formatter},
    mem::size_of,
    net::SocketAddr,
    time::Duration,
};

use call_api::{CallCommand, CallSnapshot};
use call_bridge::{BridgeError, BridgeEvent, BridgeRegistry, BridgeRegistryConfig, BridgeSnapshot};
use call_core::{BridgeId, CallId, LegId, LifecycleEvent, StreamId};
use call_engine::{CallEngine, EngineError, EngineOutput, SendAction};
use media_core::{
    AudioFrame, MediaSession, MediaSessionError, MediaSessionStats, PushOutcome, ReceivedMedia,
};
use rtcp::RtcpPacket;
use sip_parser::ParseError;
use sip_transaction::TransportReliability;
use sip_types::SipMessage;

const DEFAULT_MAX_STEPS: usize = 4_096;
const DEFAULT_MAX_WIRE_BYTES: usize = 65_535;
const DEFAULT_MAX_REPORTED_CALLS: usize = 4_096;
const DEFAULT_MAX_REPORTED_BRIDGES: usize = 4_096;
const MAX_SCENARIO_NAME_BYTES: usize = 256;

/// Resource bounds applied before and during one scenario replay.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReplayConfig {
    /// Maximum operations accepted in one scenario.
    pub max_steps: usize,
    /// Maximum SIP or RTP fixture bytes accepted by one operation.
    pub max_wire_bytes: usize,
    /// Maximum final call snapshots retained in the report.
    pub max_reported_calls: usize,
    /// Maximum final bridge snapshots retained in the report.
    pub max_reported_bridges: usize,
}

impl Default for ReplayConfig {
    fn default() -> Self {
        Self {
            max_steps: DEFAULT_MAX_STEPS,
            max_wire_bytes: DEFAULT_MAX_WIRE_BYTES,
            max_reported_calls: DEFAULT_MAX_REPORTED_CALLS,
            max_reported_bridges: DEFAULT_MAX_REPORTED_BRIDGES,
        }
    }
}

impl ReplayConfig {
    fn validate(self) -> Result<Self, ReplayError> {
        if self.max_steps == 0
            || self.max_wire_bytes == 0
            || self.max_reported_calls == 0
            || self.max_reported_bridges == 0
        {
            return Err(ReplayError::InvalidConfig);
        }
        Ok(self)
    }
}

/// One deterministic operation in an offline call scenario.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScenarioStep {
    /// Parse and deliver an inbound SIP request or response.
    ReceiveSip {
        /// Explicit monotonic scenario time.
        at: Duration,
        /// Observed signaling peer.
        source: SocketAddr,
        /// Transport reliability used by transaction timers.
        reliability: TransportReliability,
        /// Serialized SIP message fixture.
        wire: Vec<u8>,
    },
    /// Parse and originate an outbound SIP INVITE.
    OriginateSip {
        /// Explicit monotonic scenario time.
        at: Duration,
        /// Selected signaling peer.
        destination: SocketAddr,
        /// Transport reliability used by transaction timers.
        reliability: TransportReliability,
        /// Serialized SIP INVITE fixture.
        wire: Vec<u8>,
    },
    /// Emit a provisional or final response for an inbound call.
    RespondToInvite {
        /// Explicit monotonic scenario time.
        at: Duration,
        /// Stable application call identifier.
        call_id: CallId,
        /// SIP response status.
        status_code: u16,
        /// SIP reason phrase.
        reason: String,
        /// Optional response body, normally SDP.
        body: Vec<u8>,
    },
    /// Apply a provider-neutral call-control operation.
    ApplyCallCommand {
        /// Stable application call identifier.
        call_id: CallId,
        /// Command exposed through the internal API contract.
        command: CallCommand,
    },
    /// Remove one terminal call and all signaling resources owned by it.
    ReclaimTerminalCall {
        /// Stable application call identifier.
        call_id: CallId,
    },
    /// Create an AI-backed bridge for one stable inbound caller.
    CreateBridge {
        /// Stable inbound application call identity.
        caller_call_id: CallId,
        /// Stable inbound signaling/media leg identity.
        caller_leg_id: LegId,
        /// Retained AI media stream used for initial routing and fail-back.
        ai_stream_id: StreamId,
    },
    /// Begin establishing a server-originated human second leg.
    BeginHumanLeg {
        /// Bridge that owns the stable inbound caller.
        bridge_id: BridgeId,
        /// Outbound human application call identity.
        call_id: CallId,
        /// Outbound human signaling/media leg identity.
        leg_id: LegId,
    },
    /// Activate the pending human leg.
    CompleteHumanLeg {
        /// Bridge whose pending human leg connected.
        bridge_id: BridgeId,
    },
    /// Fail a pending or active human leg and restore AI routing.
    FailHumanLeg {
        /// Bridge whose human leg failed.
        bridge_id: BridgeId,
    },
    /// Explicitly switch an active human bridge back to AI.
    ResumeBridgeAi {
        /// Bridge to restore to its retained AI stream.
        bridge_id: BridgeId,
    },
    /// End all forwarding for one bridge.
    EndBridge {
        /// Bridge to move into terminal state.
        bridge_id: BridgeId,
    },
    /// Remove one terminal bridge and release all of its endpoint identities.
    ReclaimTerminalBridge {
        /// Terminal bridge to reclaim.
        bridge_id: BridgeId,
    },
    /// Advance transaction timers without sleeping or consulting wall time.
    Poll {
        /// Explicit monotonic scenario time.
        at: Duration,
    },
    /// Deliver one serialized RTP packet to the configured media session.
    ReceiveRtp {
        /// Explicit packet-arrival time.
        at: Duration,
        /// Observed media peer.
        source: SocketAddr,
        /// Serialized RTP packet fixture.
        wire: Vec<u8>,
    },
    /// Deliver one serialized RTCP datagram to the configured media session.
    ReceiveRtcp {
        /// Explicit datagram-arrival time.
        at: Duration,
        /// Observed media-control peer.
        source: SocketAddr,
        /// Serialized RTCP datagram fixture.
        wire: Vec<u8>,
    },
    /// Queue one decoded AI audio frame toward RTP output.
    PushAiAudio {
        /// Decoded audio fixture.
        frame: AudioFrame,
    },
    /// Serialize the next queued AI frame as RTP.
    EmitAudioRtp {
        /// Marker bit for the emitted RTP packet.
        marker: bool,
    },
}

impl ScenarioStep {
    fn at(&self) -> Option<Duration> {
        match self {
            Self::ReceiveSip { at, .. }
            | Self::OriginateSip { at, .. }
            | Self::RespondToInvite { at, .. }
            | Self::Poll { at }
            | Self::ReceiveRtp { at, .. }
            | Self::ReceiveRtcp { at, .. } => Some(*at),
            Self::ApplyCallCommand { .. }
            | Self::ReclaimTerminalCall { .. }
            | Self::CreateBridge { .. }
            | Self::BeginHumanLeg { .. }
            | Self::CompleteHumanLeg { .. }
            | Self::FailHumanLeg { .. }
            | Self::ResumeBridgeAi { .. }
            | Self::EndBridge { .. }
            | Self::ReclaimTerminalBridge { .. }
            | Self::PushAiAudio { .. }
            | Self::EmitAudioRtp { .. } => None,
        }
    }

    fn fixture_len(&self) -> Option<usize> {
        match self {
            Self::ReceiveSip { wire, .. }
            | Self::OriginateSip { wire, .. }
            | Self::ReceiveRtp { wire, .. }
            | Self::ReceiveRtcp { wire, .. } => Some(wire.len()),
            Self::RespondToInvite { reason, body, .. } => {
                reason.len().checked_add(body.len()).or(Some(usize::MAX))
            }
            Self::PushAiAudio { frame } => frame
                .samples
                .len()
                .checked_mul(size_of::<i16>())
                .or(Some(usize::MAX)),
            Self::ApplyCallCommand { .. }
            | Self::ReclaimTerminalCall { .. }
            | Self::CreateBridge { .. }
            | Self::BeginHumanLeg { .. }
            | Self::CompleteHumanLeg { .. }
            | Self::FailHumanLeg { .. }
            | Self::ResumeBridgeAi { .. }
            | Self::EndBridge { .. }
            | Self::ReclaimTerminalBridge { .. }
            | Self::Poll { .. }
            | Self::EmitAudioRtp { .. } => None,
        }
    }
}

/// Named sequence of deterministic replay operations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Scenario {
    name: String,
    steps: Vec<ScenarioStep>,
}

impl Scenario {
    /// Creates a scenario. Structural bounds are checked by [`ReplayRunner`].
    #[must_use]
    pub fn new(name: impl Into<String>, steps: Vec<ScenarioStep>) -> Self {
        Self {
            name: name.into(),
            steps,
        }
    }

    /// Returns the stable fixture name used in diagnostics.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns operations in execution order.
    #[must_use]
    pub fn steps(&self) -> &[ScenarioStep] {
        &self.steps
    }
}

/// Observable result of exactly one scenario operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StepOutcome {
    /// Signaling actions and API lifecycle events emitted by the engine.
    Engine {
        /// Stable call identifier allocated by an originate operation, if any.
        originated_call: Option<CallId>,
        /// Outbound signaling actions in emission order.
        actions: Vec<SendAction>,
        /// Lifecycle events in emission order.
        events: Vec<LifecycleEvent>,
    },
    /// Snapshot removed by explicit terminal resource reclamation.
    CallReclaimed(CallSnapshot),
    /// Ordered event emitted by one bridge transition.
    BridgeTransition(BridgeEvent),
    /// Snapshot removed by explicit terminal bridge reclamation.
    BridgeReclaimed(BridgeSnapshot),
    /// Decoded media outcome from one RTP packet.
    MediaReceived(ReceivedMedia),
    /// Parsed packets from one RTCP compound datagram.
    RtcpReceived(Vec<RtcpPacket>),
    /// Backpressure result from queueing one AI frame.
    AiAudioQueued(PushOutcome),
    /// Serialized RTP output, or `None` when no AI frame was queued.
    AudioRtpEmitted(Option<Vec<u8>>),
}

/// Indexed outcome retained for deterministic assertions and diagnostics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StepReport {
    /// Zero-based operation index.
    pub index: usize,
    /// Observable operation result.
    pub outcome: StepOutcome,
}

/// Complete deterministic result of one replay.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayReport {
    /// Scenario fixture name.
    pub scenario: String,
    /// Per-step outcomes in execution order.
    pub steps: Vec<StepReport>,
    /// Final call snapshots ordered by stable application ID.
    pub calls: Vec<CallSnapshot>,
    /// Final bridge snapshots ordered by stable bridge ID.
    pub bridges: Vec<BridgeSnapshot>,
    /// Remaining live SIP client/server transactions.
    pub transaction_count: usize,
    /// Final media counters when a media session was configured.
    pub media: Option<MediaSessionStats>,
}

impl ReplayReport {
    /// Returns all lifecycle events in scenario emission order.
    #[must_use]
    pub fn events(&self) -> Vec<&LifecycleEvent> {
        self.steps
            .iter()
            .filter_map(|step| match &step.outcome {
                StepOutcome::Engine { events, .. } => Some(events.iter()),
                StepOutcome::CallReclaimed(_)
                | StepOutcome::BridgeTransition(_)
                | StepOutcome::BridgeReclaimed(_)
                | StepOutcome::MediaReceived(_)
                | StepOutcome::RtcpReceived(_)
                | StepOutcome::AiAudioQueued(_)
                | StepOutcome::AudioRtpEmitted(_) => None,
            })
            .flatten()
            .collect()
    }

    /// Returns all bridge events in scenario emission order.
    #[must_use]
    pub fn bridge_events(&self) -> Vec<&BridgeEvent> {
        self.steps
            .iter()
            .filter_map(|step| match &step.outcome {
                StepOutcome::BridgeTransition(event) => Some(event),
                StepOutcome::Engine { .. }
                | StepOutcome::CallReclaimed(_)
                | StepOutcome::BridgeReclaimed(_)
                | StepOutcome::MediaReceived(_)
                | StepOutcome::RtcpReceived(_)
                | StepOutcome::AiAudioQueued(_)
                | StepOutcome::AudioRtpEmitted(_) => None,
            })
            .collect()
    }

    /// Returns all outbound signaling actions in scenario emission order.
    #[must_use]
    pub fn actions(&self) -> Vec<&SendAction> {
        self.steps
            .iter()
            .filter_map(|step| match &step.outcome {
                StepOutcome::Engine { actions, .. } => Some(actions.iter()),
                StepOutcome::CallReclaimed(_)
                | StepOutcome::BridgeTransition(_)
                | StepOutcome::BridgeReclaimed(_)
                | StepOutcome::MediaReceived(_)
                | StepOutcome::RtcpReceived(_)
                | StepOutcome::AiAudioQueued(_)
                | StepOutcome::AudioRtpEmitted(_) => None,
            })
            .flatten()
            .collect()
    }
}

/// Bounded deterministic executor for signaling, media, and bridge fixtures.
#[derive(Clone, Debug)]
pub struct ReplayRunner {
    config: ReplayConfig,
    engine: CallEngine,
    bridges: BridgeRegistry,
    media: Option<MediaSession>,
}

impl ReplayRunner {
    /// Creates a replay runner with signaling and a default bounded bridge registry.
    ///
    /// # Errors
    ///
    /// Returns an error when any replay bound is zero.
    pub fn new(config: ReplayConfig, engine: CallEngine) -> Result<Self, ReplayError> {
        Ok(Self {
            config: config.validate()?,
            engine,
            bridges: BridgeRegistry::new(BridgeRegistryConfig::default())?,
            media: None,
        })
    }

    /// Replaces the default bridge registry used by bridge operations.
    #[must_use]
    pub fn with_bridges(mut self, bridges: BridgeRegistry) -> Self {
        self.bridges = bridges;
        self
    }

    /// Adds the media session used by RTP and AI-audio operations.
    #[must_use]
    pub fn with_media(mut self, media: MediaSession) -> Self {
        self.media = Some(media);
        self
    }

    /// Replays a complete scenario without sockets, sleeps, or wall-clock time.
    ///
    /// # Errors
    ///
    /// Returns a step-indexed error for invalid bounds, non-monotonic time,
    /// malformed wire input, missing media configuration, or engine/media/bridge
    /// rejection. The runner stops at the first rejected operation and commits
    /// no signaling or media state unless the complete scenario succeeds.
    pub fn run(&mut self, scenario: &Scenario) -> Result<ReplayReport, ReplayError> {
        let mut working = self.clone();
        let report = working.run_inner(scenario)?;
        *self = working;
        Ok(report)
    }

    fn run_inner(&mut self, scenario: &Scenario) -> Result<ReplayReport, ReplayError> {
        self.validate_scenario(scenario)?;
        let mut reports = Vec::with_capacity(scenario.steps.len());
        let mut last_time = None;

        for (index, step) in scenario.steps.iter().enumerate() {
            if let Some(at) = step.at() {
                if last_time.is_some_and(|previous| at < previous) {
                    return Err(ReplayError::Step {
                        index,
                        source: StepError::NonMonotonicTime,
                    });
                }
                last_time = Some(at);
            }
            let outcome = self
                .run_step(step)
                .map_err(|source| ReplayError::Step { index, source })?;
            reports.push(StepReport { index, outcome });
        }

        Ok(ReplayReport {
            scenario: scenario.name.clone(),
            steps: reports,
            calls: self.engine.list(self.config.max_reported_calls)?,
            bridges: self.bridges.list(self.config.max_reported_bridges)?,
            transaction_count: self.engine.transaction_count(),
            media: self.media.as_ref().map(MediaSession::stats),
        })
    }

    fn validate_scenario(&self, scenario: &Scenario) -> Result<(), ReplayError> {
        if scenario.name.is_empty() || scenario.name.len() > MAX_SCENARIO_NAME_BYTES {
            return Err(ReplayError::InvalidScenarioName);
        }
        if scenario.steps.len() > self.config.max_steps {
            return Err(ReplayError::TooManySteps {
                actual: scenario.steps.len(),
                maximum: self.config.max_steps,
            });
        }
        for (index, step) in scenario.steps.iter().enumerate() {
            if let Some(actual) = step.fixture_len() {
                if actual > self.config.max_wire_bytes {
                    return Err(ReplayError::FixtureTooLarge {
                        index,
                        actual,
                        maximum: self.config.max_wire_bytes,
                    });
                }
            }
        }
        Ok(())
    }

    fn run_step(&mut self, step: &ScenarioStep) -> Result<StepOutcome, StepError> {
        match step {
            ScenarioStep::ReceiveSip {
                at,
                source,
                reliability,
                wire,
            } => {
                let message = sip_parser::parse(wire)?;
                let output = match message {
                    SipMessage::Request(request) => {
                        self.engine
                            .receive_request(*source, request, *at, *reliability)?
                    }
                    SipMessage::Response(response) => {
                        self.engine.receive_response(response, *at)?
                    }
                };
                Ok(engine_outcome(None, output))
            }
            ScenarioStep::OriginateSip {
                at,
                destination,
                reliability,
                wire,
            } => {
                let SipMessage::Request(request) = sip_parser::parse(wire)? else {
                    return Err(StepError::ExpectedRequest);
                };
                let (call_id, output) =
                    self.engine
                        .originate(request, *destination, *at, *reliability)?;
                Ok(engine_outcome(Some(call_id), output))
            }
            ScenarioStep::RespondToInvite {
                at,
                call_id,
                status_code,
                reason,
                body,
            } => Ok(engine_outcome(
                None,
                self.engine.respond_to_invite(
                    call_id,
                    *status_code,
                    reason.clone(),
                    body.clone(),
                    *at,
                )?,
            )),
            ScenarioStep::ApplyCallCommand { call_id, command } => Ok(engine_outcome(
                None,
                self.engine.apply_call_command(call_id, *command)?,
            )),
            ScenarioStep::ReclaimTerminalCall { call_id } => Ok(StepOutcome::CallReclaimed(
                self.engine.reclaim_terminal_call(call_id)?,
            )),
            ScenarioStep::CreateBridge {
                caller_call_id,
                caller_leg_id,
                ai_stream_id,
            } => self.create_bridge(caller_call_id, caller_leg_id, ai_stream_id),
            ScenarioStep::BeginHumanLeg {
                bridge_id,
                call_id,
                leg_id,
            } => self.begin_human_leg(bridge_id, call_id, leg_id),
            ScenarioStep::CompleteHumanLeg { bridge_id } => self.complete_human_leg(bridge_id),
            ScenarioStep::FailHumanLeg { bridge_id } => self.fail_human_leg(bridge_id),
            ScenarioStep::ResumeBridgeAi { bridge_id } => self.resume_bridge_ai(bridge_id),
            ScenarioStep::EndBridge { bridge_id } => self.end_bridge(bridge_id),
            ScenarioStep::ReclaimTerminalBridge { bridge_id } => Ok(StepOutcome::BridgeReclaimed(
                self.bridges.remove_terminal(bridge_id)?,
            )),
            ScenarioStep::Poll { at } => Ok(engine_outcome(None, self.engine.poll(*at)?)),
            ScenarioStep::ReceiveRtp { at, source, wire } => {
                let media = self.media.as_mut().ok_or(StepError::MediaNotConfigured)?;
                Ok(StepOutcome::MediaReceived(
                    media.receive_rtp_from(wire, *source, *at)?,
                ))
            }
            ScenarioStep::ReceiveRtcp { at, source, wire } => {
                let media = self.media.as_mut().ok_or(StepError::MediaNotConfigured)?;
                Ok(StepOutcome::RtcpReceived(
                    media.receive_rtcp_from(wire, *source, *at)?,
                ))
            }
            ScenarioStep::PushAiAudio { frame } => {
                let media = self.media.as_mut().ok_or(StepError::MediaNotConfigured)?;
                Ok(StepOutcome::AiAudioQueued(
                    media.push_from_ai(frame.clone()),
                ))
            }
            ScenarioStep::EmitAudioRtp { marker } => {
                let media = self.media.as_mut().ok_or(StepError::MediaNotConfigured)?;
                Ok(StepOutcome::AudioRtpEmitted(media.next_audio_rtp(*marker)?))
            }
        }
    }

    fn create_bridge(
        &mut self,
        caller_call_id: &CallId,
        caller_leg_id: &LegId,
        ai_stream_id: &StreamId,
    ) -> Result<StepOutcome, StepError> {
        let (_, event) = self.bridges.create_ai(
            caller_call_id.clone(),
            caller_leg_id.clone(),
            ai_stream_id.clone(),
        )?;
        Ok(StepOutcome::BridgeTransition(event))
    }

    fn begin_human_leg(
        &mut self,
        bridge_id: &BridgeId,
        call_id: &CallId,
        leg_id: &LegId,
    ) -> Result<StepOutcome, StepError> {
        Ok(StepOutcome::BridgeTransition(self.bridges.begin_human(
            bridge_id,
            call_id.clone(),
            leg_id.clone(),
        )?))
    }

    fn complete_human_leg(&mut self, id: &BridgeId) -> Result<StepOutcome, StepError> {
        Ok(StepOutcome::BridgeTransition(
            self.bridges.complete_human(id)?,
        ))
    }

    fn fail_human_leg(&mut self, id: &BridgeId) -> Result<StepOutcome, StepError> {
        Ok(StepOutcome::BridgeTransition(self.bridges.fail_human(id)?))
    }

    fn resume_bridge_ai(&mut self, id: &BridgeId) -> Result<StepOutcome, StepError> {
        Ok(StepOutcome::BridgeTransition(self.bridges.resume_ai(id)?))
    }

    fn end_bridge(&mut self, id: &BridgeId) -> Result<StepOutcome, StepError> {
        Ok(StepOutcome::BridgeTransition(self.bridges.end(id)?))
    }
}

fn engine_outcome(originated_call: Option<CallId>, output: EngineOutput) -> StepOutcome {
    let (actions, events) = output.into_parts();
    StepOutcome::Engine {
        originated_call,
        actions,
        events,
    }
}

/// Failure to validate or execute a complete scenario.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReplayError {
    /// At least one replay resource bound was zero.
    InvalidConfig,
    /// The fixture name was empty or exceeded its bound.
    InvalidScenarioName,
    /// The scenario exceeded its configured operation bound.
    TooManySteps {
        /// Supplied operation count.
        actual: usize,
        /// Configured maximum.
        maximum: usize,
    },
    /// One wire/body/audio fixture exceeded its configured byte bound.
    FixtureTooLarge {
        /// Zero-based operation index.
        index: usize,
        /// Supplied byte count.
        actual: usize,
        /// Configured maximum.
        maximum: usize,
    },
    /// One operation was rejected.
    Step {
        /// Zero-based operation index.
        index: usize,
        /// Contextual operation failure.
        source: StepError,
    },
    /// Final call snapshot collection failed.
    Engine(EngineError),
    /// Bridge setup or final snapshot collection failed.
    Bridge(BridgeError),
}

impl Display for ReplayError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfig => formatter.write_str("replay bounds must be non-zero"),
            Self::InvalidScenarioName => {
                formatter.write_str("scenario name must be non-empty and at most 256 bytes")
            }
            Self::TooManySteps { actual, maximum } => {
                write!(
                    formatter,
                    "scenario has {actual} steps, maximum is {maximum}"
                )
            }
            Self::FixtureTooLarge {
                index,
                actual,
                maximum,
            } => write!(
                formatter,
                "scenario step {index} has {actual} fixture bytes, maximum is {maximum}"
            ),
            Self::Step { index, source } => {
                write!(formatter, "scenario step {index} failed: {source}")
            }
            Self::Engine(error) => Display::fmt(error, formatter),
            Self::Bridge(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for ReplayError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Step { source, .. } => Some(source),
            Self::Engine(error) => Some(error),
            Self::Bridge(error) => Some(error),
            Self::InvalidConfig
            | Self::InvalidScenarioName
            | Self::TooManySteps { .. }
            | Self::FixtureTooLarge { .. } => None,
        }
    }
}

impl From<EngineError> for ReplayError {
    fn from(error: EngineError) -> Self {
        Self::Engine(error)
    }
}

impl From<BridgeError> for ReplayError {
    fn from(error: BridgeError) -> Self {
        Self::Bridge(error)
    }
}

/// Contextual failure from one scenario operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StepError {
    /// A step timestamp moved backward relative to an earlier timed step.
    NonMonotonicTime,
    /// An originate operation parsed a SIP response instead of a request.
    ExpectedRequest,
    /// A media operation was used without a configured media session.
    MediaNotConfigured,
    /// SIP wire parsing failed.
    SipParse(ParseError),
    /// SIP transaction/dialog/call orchestration failed.
    Engine(EngineError),
    /// Bridge state or resource validation failed.
    Bridge(BridgeError),
    /// RTP/media processing failed.
    Media(MediaSessionError),
}

impl Display for StepError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonMonotonicTime => formatter.write_str("scenario time moved backward"),
            Self::ExpectedRequest => formatter.write_str("originate step requires a SIP request"),
            Self::MediaNotConfigured => {
                formatter.write_str("scenario uses media without a configured media session")
            }
            Self::SipParse(error) => Display::fmt(error, formatter),
            Self::Engine(error) => Display::fmt(error, formatter),
            Self::Bridge(error) => Display::fmt(error, formatter),
            Self::Media(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for StepError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::SipParse(error) => Some(error),
            Self::Engine(error) => Some(error),
            Self::Bridge(error) => Some(error),
            Self::Media(error) => Some(error),
            Self::NonMonotonicTime | Self::ExpectedRequest | Self::MediaNotConfigured => None,
        }
    }
}

impl From<ParseError> for StepError {
    fn from(error: ParseError) -> Self {
        Self::SipParse(error)
    }
}

impl From<EngineError> for StepError {
    fn from(error: EngineError) -> Self {
        Self::Engine(error)
    }
}

impl From<BridgeError> for StepError {
    fn from(error: BridgeError) -> Self {
        Self::Bridge(error)
    }
}

impl From<MediaSessionError> for StepError {
    fn from(error: MediaSessionError) -> Self {
        Self::Media(error)
    }
}
