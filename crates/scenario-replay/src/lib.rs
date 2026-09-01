//! Deterministic, offline SIP and media scenario replay.
//!
//! The replay boundary consumes owned wire fixtures and explicit monotonic
//! timestamps. It owns no sockets, sleeps, wall clock, credentials, or provider
//! configuration, so synthetic scenarios and later sanitized captures can use
//! the same execution path.

use std::{
    error::Error,
    fmt::{Display, Formatter},
    mem::size_of,
    net::SocketAddr,
    time::Duration,
};

use call_api::{CallCommand, CallSnapshot};
use call_core::{CallId, LifecycleEvent};
use call_engine::{CallEngine, EngineError, EngineOutput, SendAction};
use media_core::{
    AudioFrame, MediaSession, MediaSessionError, MediaSessionStats, PushOutcome, ReceivedMedia,
};
use sip_parser::ParseError;
use sip_transaction::TransportReliability;
use sip_types::SipMessage;

const DEFAULT_MAX_STEPS: usize = 4_096;
const DEFAULT_MAX_WIRE_BYTES: usize = 65_535;
const DEFAULT_MAX_REPORTED_CALLS: usize = 4_096;
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
}

impl Default for ReplayConfig {
    fn default() -> Self {
        Self {
            max_steps: DEFAULT_MAX_STEPS,
            max_wire_bytes: DEFAULT_MAX_WIRE_BYTES,
            max_reported_calls: DEFAULT_MAX_REPORTED_CALLS,
        }
    }
}

impl ReplayConfig {
    fn validate(self) -> Result<Self, ReplayError> {
        if self.max_steps == 0 || self.max_wire_bytes == 0 || self.max_reported_calls == 0 {
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
            | Self::ReceiveRtp { at, .. } => Some(*at),
            Self::ApplyCallCommand { .. }
            | Self::PushAiAudio { .. }
            | Self::EmitAudioRtp { .. } => None,
        }
    }

    fn fixture_len(&self) -> Option<usize> {
        match self {
            Self::ReceiveSip { wire, .. }
            | Self::OriginateSip { wire, .. }
            | Self::ReceiveRtp { wire, .. } => Some(wire.len()),
            Self::RespondToInvite { reason, body, .. } => {
                reason.len().checked_add(body.len()).or(Some(usize::MAX))
            }
            Self::PushAiAudio { frame } => frame
                .samples
                .len()
                .checked_mul(size_of::<i16>())
                .or(Some(usize::MAX)),
            Self::ApplyCallCommand { .. } | Self::Poll { .. } | Self::EmitAudioRtp { .. } => None,
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
    /// Decoded media outcome from one RTP packet.
    MediaReceived(ReceivedMedia),
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
                StepOutcome::MediaReceived(_)
                | StepOutcome::AiAudioQueued(_)
                | StepOutcome::AudioRtpEmitted(_) => None,
            })
            .flatten()
            .collect()
    }

    /// Returns all outbound signaling actions in scenario emission order.
    #[must_use]
    pub fn actions(&self) -> Vec<&SendAction> {
        self.steps
            .iter()
            .filter_map(|step| match &step.outcome {
                StepOutcome::Engine { actions, .. } => Some(actions.iter()),
                StepOutcome::MediaReceived(_)
                | StepOutcome::AiAudioQueued(_)
                | StepOutcome::AudioRtpEmitted(_) => None,
            })
            .flatten()
            .collect()
    }
}

/// Bounded deterministic executor for signaling and media fixtures.
#[derive(Clone, Debug)]
pub struct ReplayRunner {
    config: ReplayConfig,
    engine: CallEngine,
    media: Option<MediaSession>,
}

impl ReplayRunner {
    /// Creates a signaling-only replay runner.
    ///
    /// # Errors
    ///
    /// Returns an error when any replay bound is zero.
    pub fn new(config: ReplayConfig, engine: CallEngine) -> Result<Self, ReplayError> {
        Ok(Self {
            config: config.validate()?,
            engine,
            media: None,
        })
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
    /// malformed wire input, missing media configuration, or engine/media
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
            ScenarioStep::Poll { at } => Ok(engine_outcome(None, self.engine.poll(*at)?)),
            ScenarioStep::ReceiveRtp { at, source, wire } => {
                let media = self.media.as_mut().ok_or(StepError::MediaNotConfigured)?;
                Ok(StepOutcome::MediaReceived(
                    media.receive_rtp_from(wire, *source, *at)?,
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
        }
    }
}

impl Error for ReplayError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Step { source, .. } => Some(source),
            Self::Engine(error) => Some(error),
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
            Self::Media(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for StepError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::SipParse(error) => Some(error),
            Self::Engine(error) => Some(error),
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

impl From<MediaSessionError> for StepError {
    fn from(error: MediaSessionError) -> Self {
        Self::Media(error)
    }
}
