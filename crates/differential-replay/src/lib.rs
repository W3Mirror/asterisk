//! Bounded semantic comparison for Rust and future Asterisk/provider replays.
//!
//! The normalizer converts a [`scenario_replay::ReplayReport`] into ordered,
//! stable facts. Application IDs, SIP Call-IDs, destination addresses, dialog
//! tags, transaction branches, SDP addresses/ports, and wall-clock values are
//! omitted, replaced by first-seen aliases, or reduced to semantic presence
//! flags. This preserves
//! response order, state, negotiated media, events, media counters, and cleanup
//! while avoiding false differences from environment-owned values.

use std::{
    collections::HashMap,
    error::Error,
    fmt::{Display, Formatter},
    hash::Hash,
    net::SocketAddr,
};

use call_api::{CallSnapshot, NegotiatedAudio};
use call_bridge::{BridgeEventKind, BridgeSnapshot, BridgeState};
use call_core::{BridgeId, CallEventKind, CallId, CallState};
use scenario_replay::{ReplayReport, StepOutcome};
use sdp::{Codec, Direction, SessionDescription};
use sip_types::{Headers, SipMessage};

const DEFAULT_MAX_FACTS: usize = 16_384;
const DEFAULT_MAX_FACT_BYTES: usize = 4_096;
const DEFAULT_MAX_FIXTURE_BYTES: usize = 4 * 1024 * 1024;
const DEFAULT_MAX_DIFFERENCES: usize = 64;

/// Bounds applied while converting one replay report into semantic facts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NormalizationConfig {
    /// Maximum facts retained for one scenario.
    pub max_facts: usize,
    /// Maximum UTF-8 bytes retained in one fact.
    pub max_fact_bytes: usize,
}

impl Default for NormalizationConfig {
    fn default() -> Self {
        Self {
            max_facts: DEFAULT_MAX_FACTS,
            max_fact_bytes: DEFAULT_MAX_FACT_BYTES,
        }
    }
}

/// Bounds applied while parsing one checked-in oracle fixture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FixtureConfig {
    /// Maximum complete fixture size.
    pub max_bytes: usize,
    /// Maximum semantic fact count.
    pub max_facts: usize,
    /// Maximum bytes in one fixture line.
    pub max_line_bytes: usize,
}

impl Default for FixtureConfig {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_MAX_FIXTURE_BYTES,
            max_facts: DEFAULT_MAX_FACTS,
            max_line_bytes: DEFAULT_MAX_FACT_BYTES + 5,
        }
    }
}

/// Bounds applied while reporting semantic differences.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ComparisonConfig {
    /// Maximum mismatches returned to the caller.
    pub max_differences: usize,
}

impl Default for ComparisonConfig {
    fn default() -> Self {
        Self {
            max_differences: DEFAULT_MAX_DIFFERENCES,
        }
    }
}

/// Stable normalized observation accepted from Rust or a converted oracle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NormalizedObservation {
    scenario: String,
    facts: Vec<String>,
}

impl NormalizedObservation {
    /// Returns the stable scenario slug.
    #[must_use]
    pub fn scenario(&self) -> &str {
        &self.scenario
    }

    /// Returns ordered semantic facts.
    #[must_use]
    pub fn facts(&self) -> &[String] {
        &self.facts
    }

    /// Serializes the observation into the bounded oracle fixture format.
    #[must_use]
    pub fn to_fixture(&self) -> String {
        let mut output = format!("version\t1\nscenario\t{}\n", self.scenario);
        for fact in &self.facts {
            output.push_str("fact\t");
            output.push_str(fact);
            output.push('\n');
        }
        output
    }
}

/// One indexed semantic mismatch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Difference {
    /// Zero-based fact index, or `None` for the scenario identity.
    pub fact_index: Option<usize>,
    /// Oracle value, absent when Rust emitted an extra fact.
    pub expected: Option<String>,
    /// Rust value, absent when Rust omitted an oracle fact.
    pub actual: Option<String>,
}

/// Bounded result of comparing one Rust observation to one oracle fixture.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Comparison {
    /// Whether scenario identity and every semantic fact matched.
    pub matched: bool,
    /// Ordered mismatches, capped by [`ComparisonConfig`].
    pub differences: Vec<Difference>,
    /// Total mismatch count before diagnostic truncation.
    pub total_differences: usize,
}

/// Failure to normalize, parse, or configure a differential comparison.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DifferentialError {
    /// At least one configured bound was zero.
    InvalidConfig,
    /// Scenario identity was empty or not a safe fixture slug.
    InvalidScenario,
    /// The normalized report exceeded its fact-count bound.
    TooManyFacts {
        /// Observed fact count.
        actual: usize,
        /// Configured maximum.
        maximum: usize,
    },
    /// One normalized fact exceeded its byte bound.
    FactTooLong {
        /// Zero-based fact index.
        index: usize,
        /// Observed bytes.
        actual: usize,
        /// Configured maximum.
        maximum: usize,
    },
    /// Oracle fixture exceeded its complete byte bound.
    FixtureTooLarge {
        /// Observed bytes.
        actual: usize,
        /// Configured maximum.
        maximum: usize,
    },
    /// One oracle line exceeded its byte bound.
    FixtureLineTooLong {
        /// One-based line number.
        line: usize,
        /// Configured maximum.
        maximum: usize,
    },
    /// Oracle fixture syntax or semantic category was invalid.
    InvalidFixture {
        /// One-based line number.
        line: usize,
        /// Stable reason suitable for diagnostics.
        reason: &'static str,
    },
}

impl Display for DifferentialError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfig => formatter.write_str("differential bounds must be non-zero"),
            Self::InvalidScenario => formatter.write_str("scenario must be a safe non-empty slug"),
            Self::TooManyFacts { actual, maximum } => {
                write!(
                    formatter,
                    "observation has {actual} facts, maximum is {maximum}"
                )
            }
            Self::FactTooLong {
                index,
                actual,
                maximum,
            } => write!(
                formatter,
                "observation fact {index} has {actual} bytes, maximum is {maximum}"
            ),
            Self::FixtureTooLarge { actual, maximum } => write!(
                formatter,
                "oracle fixture has {actual} bytes, maximum is {maximum}"
            ),
            Self::FixtureLineTooLong { line, maximum } => write!(
                formatter,
                "oracle fixture line {line} exceeds {maximum} bytes"
            ),
            Self::InvalidFixture { line, reason } => {
                write!(formatter, "oracle fixture line {line} is invalid: {reason}")
            }
        }
    }
}

impl Error for DifferentialError {}

/// Normalizes a Rust replay report into ordered environment-independent facts.
///
/// # Errors
///
/// Returns an error when bounds are zero, the scenario is not a safe slug, or
/// generated facts exceed configured count/size limits.
pub fn normalize_replay(
    report: &ReplayReport,
    config: NormalizationConfig,
) -> Result<NormalizedObservation, DifferentialError> {
    validate_normalization(config)?;
    validate_scenario(&report.scenario)?;
    let mut normalizer = Normalizer::new(config);
    normalizer.push("timing order-only".to_owned())?;
    normalizer.add_sip_actions(report)?;
    normalizer.add_events(report)?;
    normalizer.add_bridges_events(report)?;
    normalizer.add_calls(report)?;
    normalizer.add_bridges(report)?;
    normalizer.add_media(report)?;
    normalizer.add_cleanup(report)?;
    Ok(NormalizedObservation {
        scenario: report.scenario.clone(),
        facts: normalizer.facts,
    })
}

/// Parses a bounded normalized oracle fixture.
///
/// Converted sanitized Asterisk/provider captures must emit this same format;
/// the comparator therefore does not need a capture-source-specific path.
///
/// # Errors
///
/// Returns an indexed error for invalid bounds, size, syntax, scenario, or fact
/// category.
pub fn parse_oracle_fixture(
    input: &str,
    config: FixtureConfig,
) -> Result<NormalizedObservation, DifferentialError> {
    if config.max_bytes == 0 || config.max_facts == 0 || config.max_line_bytes == 0 {
        return Err(DifferentialError::InvalidConfig);
    }
    if input.len() > config.max_bytes {
        return Err(DifferentialError::FixtureTooLarge {
            actual: input.len(),
            maximum: config.max_bytes,
        });
    }
    let lines = input.lines().collect::<Vec<_>>();
    for (index, line) in lines.iter().enumerate() {
        if line.len() > config.max_line_bytes {
            return Err(DifferentialError::FixtureLineTooLong {
                line: index + 1,
                maximum: config.max_line_bytes,
            });
        }
        if line
            .chars()
            .any(|character| character.is_control() && character != '\t')
        {
            return Err(DifferentialError::InvalidFixture {
                line: index + 1,
                reason: "control character",
            });
        }
    }
    if lines.first() != Some(&"version\t1") {
        return Err(DifferentialError::InvalidFixture {
            line: 1,
            reason: "expected version 1",
        });
    }
    let scenario = lines
        .get(1)
        .and_then(|line| line.strip_prefix("scenario\t"))
        .ok_or(DifferentialError::InvalidFixture {
            line: 2,
            reason: "expected scenario",
        })?
        .to_owned();
    validate_scenario(&scenario)?;
    let mut facts = Vec::new();
    for (index, line) in lines.iter().enumerate().skip(2) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fact = line
            .strip_prefix("fact\t")
            .ok_or(DifferentialError::InvalidFixture {
                line: index + 1,
                reason: "expected fact",
            })?;
        if !valid_fact_category(fact) {
            return Err(DifferentialError::InvalidFixture {
                line: index + 1,
                reason: "unknown fact category",
            });
        }
        facts.push(fact.to_owned());
        if facts.len() > config.max_facts {
            return Err(DifferentialError::TooManyFacts {
                actual: facts.len(),
                maximum: config.max_facts,
            });
        }
    }
    Ok(NormalizedObservation { scenario, facts })
}

/// Compares normalized Rust output with a normalized oracle observation.
///
/// # Errors
///
/// Returns an error only when the diagnostic difference bound is zero. Semantic
/// mismatches are returned as a successful [`Comparison`] value.
pub fn compare(
    actual: &NormalizedObservation,
    expected: &NormalizedObservation,
    config: ComparisonConfig,
) -> Result<Comparison, DifferentialError> {
    if config.max_differences == 0 {
        return Err(DifferentialError::InvalidConfig);
    }
    let mut differences = Vec::new();
    let mut total_differences = 0usize;
    if actual.scenario != expected.scenario {
        total_differences += 1;
        differences.push(Difference {
            fact_index: None,
            expected: Some(expected.scenario.clone()),
            actual: Some(actual.scenario.clone()),
        });
    }
    let count = actual.facts.len().max(expected.facts.len());
    for index in 0..count {
        let actual_fact = actual.facts.get(index);
        let expected_fact = expected.facts.get(index);
        if actual_fact == expected_fact {
            continue;
        }
        total_differences += 1;
        if differences.len() < config.max_differences {
            differences.push(Difference {
                fact_index: Some(index),
                expected: expected_fact.cloned(),
                actual: actual_fact.cloned(),
            });
        }
    }
    Ok(Comparison {
        matched: total_differences == 0,
        differences,
        total_differences,
    })
}

#[derive(Debug)]
struct AliasMap<T> {
    values: HashMap<T, usize>,
}

impl<T> Default for AliasMap<T> {
    fn default() -> Self {
        Self {
            values: HashMap::new(),
        }
    }
}

impl<T: Clone + Eq + Hash> AliasMap<T> {
    fn alias(&mut self, value: &T) -> usize {
        if let Some(alias) = self.values.get(value) {
            return *alias;
        }
        let alias = self.values.len() + 1;
        self.values.insert(value.clone(), alias);
        alias
    }
}

#[derive(Debug)]
struct Normalizer {
    config: NormalizationConfig,
    facts: Vec<String>,
    endpoints: AliasMap<SocketAddr>,
    sip_calls: AliasMap<String>,
    calls: AliasMap<CallId>,
    bridges: AliasMap<BridgeId>,
}

impl Normalizer {
    fn new(config: NormalizationConfig) -> Self {
        Self {
            config,
            facts: Vec::new(),
            endpoints: AliasMap::default(),
            sip_calls: AliasMap::default(),
            calls: AliasMap::default(),
            bridges: AliasMap::default(),
        }
    }

    fn push(&mut self, fact: String) -> Result<(), DifferentialError> {
        if self.facts.len() >= self.config.max_facts {
            return Err(DifferentialError::TooManyFacts {
                actual: self.facts.len() + 1,
                maximum: self.config.max_facts,
            });
        }
        if fact.len() > self.config.max_fact_bytes {
            return Err(DifferentialError::FactTooLong {
                index: self.facts.len(),
                actual: fact.len(),
                maximum: self.config.max_fact_bytes,
            });
        }
        self.facts.push(fact);
        Ok(())
    }

    fn add_sip_actions(&mut self, report: &ReplayReport) -> Result<(), DifferentialError> {
        for (index, action) in report.actions().into_iter().enumerate() {
            let endpoint = self.endpoints.alias(&action.destination);
            let message = self.normalize_sip(&action.message);
            self.push(format!("sip {} endpoint-{endpoint} {message}", index + 1))?;
        }
        Ok(())
    }

    fn normalize_sip(&mut self, message: &SipMessage) -> String {
        match message {
            SipMessage::Request(request) => format!(
                "request {} cseq={} sip-call-{} {}",
                request.method.as_str(),
                cseq(&request.headers),
                self.sip_call_alias(&request.headers),
                normalize_body(&request.body)
            ),
            SipMessage::Response(response) => format!(
                "response {} cseq={} sip-call-{} {}",
                response.status_code,
                cseq(&response.headers),
                self.sip_call_alias(&response.headers),
                normalize_body(&response.body)
            ),
        }
    }

    fn sip_call_alias(&mut self, headers: &Headers) -> usize {
        self.sip_calls
            .alias(&headers.get("Call-ID").unwrap_or("missing").to_owned())
    }

    fn add_events(&mut self, report: &ReplayReport) -> Result<(), DifferentialError> {
        for (index, event) in report.events().into_iter().enumerate() {
            let call = self.calls.alias(&event.call_id);
            self.push(format!(
                "event {} call-{call} {}",
                index + 1,
                call_event(event.kind)
            ))?;
        }
        Ok(())
    }

    fn add_bridges_events(&mut self, report: &ReplayReport) -> Result<(), DifferentialError> {
        for (index, event) in report.bridge_events().into_iter().enumerate() {
            let bridge = self.bridges.alias(&event.bridge_id);
            self.push(format!(
                "bridge-event {} bridge-{bridge} {}",
                index + 1,
                bridge_event(event.kind)
            ))?;
        }
        Ok(())
    }

    fn add_calls(&mut self, report: &ReplayReport) -> Result<(), DifferentialError> {
        for step in &report.steps {
            if let StepOutcome::CallReclaimed(snapshot) = &step.outcome {
                self.add_call(snapshot, "reclaimed")?;
            }
        }
        for snapshot in &report.calls {
            self.add_call(snapshot, "retained")?;
        }
        Ok(())
    }

    fn add_call(
        &mut self,
        snapshot: &CallSnapshot,
        retention: &str,
    ) -> Result<(), DifferentialError> {
        let call = self.calls.alias(&snapshot.id);
        self.push(format!(
            "call call-{call} {retention} {} dialog={} {}",
            call_state(snapshot.state),
            snapshot.dialog_id.is_some(),
            normalize_negotiated(snapshot.media.as_ref())
        ))
    }

    fn add_bridges(&mut self, report: &ReplayReport) -> Result<(), DifferentialError> {
        for step in &report.steps {
            if let StepOutcome::BridgeReclaimed(snapshot) = &step.outcome {
                self.add_bridge(snapshot, "reclaimed")?;
            }
        }
        for snapshot in &report.bridges {
            self.add_bridge(snapshot, "retained")?;
        }
        Ok(())
    }

    fn add_bridge(
        &mut self,
        snapshot: &BridgeSnapshot,
        retention: &str,
    ) -> Result<(), DifferentialError> {
        let bridge = self.bridges.alias(&snapshot.id);
        let caller = self.calls.alias(&snapshot.caller_call_id);
        self.push(format!(
            "bridge bridge-{bridge} {retention} {} caller=call-{caller} pending-human={} active-human={}",
            bridge_state(snapshot.state),
            snapshot.pending_human.is_some(),
            snapshot.active_human.is_some()
        ))
    }

    fn add_media(&mut self, report: &ReplayReport) -> Result<(), DifferentialError> {
        let Some(media) = &report.media else {
            return self.push("media none".to_owned());
        };
        self.push(format!(
            "media rtp-sent={} rtp-received={} rtp-lost={} rtp-invalid={} rtp-ssrc-changes={} rtcp-sent={} rtcp-received={} rtcp-lost={} rtcp-invalid={} rtcp-ssrc-changes={} audio-in={} audio-out={} dtmf-events={} dtmf-dropped={} to-ai-pushed={} to-ai-dropped={} from-ai-pushed={} from-ai-dropped={}",
            media.rtp.packets_sent,
            media.rtp.received.packets_received,
            media.rtp.received.packets_lost,
            media.rtp.received.invalid_packets,
            media.rtp.received.ssrc_changes,
            media.rtcp.packets_sent,
            media.rtcp.packets_received,
            media.rtcp.packets_lost,
            media.rtcp.invalid_packets,
            media.rtcp.ssrc_changes,
            media.audio_frames_received,
            media.audio_frames_sent,
            media.dtmf_notifications,
            media.dropped_dtmf,
            media.bridge.to_ai.pushed,
            queue_drops(&media.bridge.to_ai),
            media.bridge.from_ai.pushed,
            queue_drops(&media.bridge.from_ai),
        ))
    }

    fn add_cleanup(&mut self, report: &ReplayReport) -> Result<(), DifferentialError> {
        let queues = report.media.as_ref().map_or_else(
            || "to-ai=na from-ai=na dtmf=na".to_owned(),
            |media| {
                format!(
                    "to-ai={} from-ai={} dtmf={}",
                    media.bridge.to_ai.depth, media.bridge.from_ai.depth, media.pending_dtmf
                )
            },
        );
        self.push(format!(
            "cleanup calls={} bridges={} transactions={} {queues}",
            report.calls.len(),
            report.bridges.len(),
            report.transaction_count
        ))
    }
}

fn validate_normalization(config: NormalizationConfig) -> Result<(), DifferentialError> {
    if config.max_facts == 0 || config.max_fact_bytes == 0 {
        return Err(DifferentialError::InvalidConfig);
    }
    Ok(())
}

fn validate_scenario(scenario: &str) -> Result<(), DifferentialError> {
    if scenario.is_empty()
        || !scenario
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(DifferentialError::InvalidScenario);
    }
    Ok(())
}

fn valid_fact_category(fact: &str) -> bool {
    [
        "timing ",
        "sip ",
        "event ",
        "bridge-event ",
        "call ",
        "bridge ",
        "media ",
        "cleanup ",
    ]
    .iter()
    .any(|prefix| fact.starts_with(prefix))
}

fn cseq(headers: &Headers) -> String {
    headers
        .get("CSeq")
        .and_then(|value| {
            let mut parts = value.split_whitespace();
            Some(format!("{}/{}", parts.next()?, parts.next()?))
        })
        .unwrap_or_else(|| "unknown/UNKNOWN".to_owned())
}

fn normalize_body(body: &[u8]) -> String {
    if body.is_empty() {
        return "body=none".to_owned();
    }
    match sdp::parse(body) {
        Ok(description) => normalize_sdp(&description),
        Err(_) => format!("body=opaque:bytes={}", body.len()),
    }
}

fn normalize_sdp(description: &SessionDescription) -> String {
    let Some(media) = description
        .media
        .iter()
        .find(|media| media.media.eq_ignore_ascii_case("audio"))
    else {
        return "body=sdp:audio=none".to_owned();
    };
    let mut codecs = media
        .codecs
        .iter()
        .filter(|codec| !codec.is_telephone_event())
        .map(normalize_codec)
        .collect::<Vec<_>>();
    codecs.sort();
    codecs.dedup();
    format!(
        "body=sdp:codecs={}:direction={}:connection={}:port={}",
        codecs.join(","),
        direction(media.effective_direction(description.direction)),
        media.connection.is_some() || description.connection.is_some(),
        media.port != 0
    )
}

fn normalize_negotiated(media: Option<&NegotiatedAudio>) -> String {
    media.map_or_else(
        || "codec=none".to_owned(),
        |media| {
            format!(
                "codec={}->{} direction={} remote-address={} remote-port={}",
                normalize_codec(&media.local_codec),
                normalize_codec(&media.remote_codec),
                direction(media.direction),
                media.remote_connection.is_some(),
                media.remote_port != 0
            )
        },
    )
}

fn normalize_codec(codec: &Codec) -> String {
    format!(
        "{}/{}/{}:fmtp={}",
        codec.name.to_ascii_uppercase(),
        codec.clock_rate,
        codec.channels,
        codec.fmtp.is_some()
    )
}

fn queue_drops(stats: &media_core::QueueStats) -> u64 {
    stats.dropped_oldest.saturating_add(stats.dropped_newest)
}

fn direction(value: Direction) -> &'static str {
    match value {
        Direction::SendRecv => "sendrecv",
        Direction::SendOnly => "sendonly",
        Direction::RecvOnly => "recvonly",
        Direction::Inactive => "inactive",
    }
}

fn call_state(value: CallState) -> &'static str {
    match value {
        CallState::Created => "created",
        CallState::Inviting => "inviting",
        CallState::Early => "early",
        CallState::Ringing => "ringing",
        CallState::Answered => "answered",
        CallState::Active => "active",
        CallState::Transferring => "transferring",
        CallState::Ending => "ending",
        CallState::Ended => "ended",
        CallState::Failed => "failed",
    }
}

fn call_event(value: CallEventKind) -> &'static str {
    match value {
        CallEventKind::Created => "created",
        CallEventKind::InviteReceived => "invite-received",
        CallEventKind::Ringing => "ringing",
        CallEventKind::EarlyMedia => "early-media",
        CallEventKind::Answered => "answered",
        CallEventKind::MediaStarted => "media-started",
        CallEventKind::Transferring => "transferring",
        CallEventKind::Transferred => "transferred",
        CallEventKind::Hangup => "hangup",
        CallEventKind::Failed => "failed",
    }
}

fn bridge_state(value: BridgeState) -> &'static str {
    match value {
        BridgeState::AiActive => "ai-active",
        BridgeState::ConnectingHuman => "connecting-human",
        BridgeState::HumanActive => "human-active",
        BridgeState::Ended => "ended",
    }
}

fn bridge_event(value: BridgeEventKind) -> &'static str {
    match value {
        BridgeEventKind::Created => "created",
        BridgeEventKind::HumanConnecting => "human-connecting",
        BridgeEventKind::HumanConnected => "human-connected",
        BridgeEventKind::HumanFailed => "human-failed",
        BridgeEventKind::AiResumed => "ai-resumed",
        BridgeEventKind::Ended => "ended",
    }
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, time::Duration};

    use call_core::CallId;
    use call_engine::{CallEngine, EngineConfig};
    use scenario_replay::{ReplayConfig, ReplayRunner, Scenario, ScenarioStep};
    use sip_transaction::TransportReliability;

    use super::*;

    const ORACLE: &str = include_str!("../tests/fixtures/inbound_cancelled.oracle");

    fn sip_fixture(value: &str) -> Vec<u8> {
        format!("{}\r\n\r\n", value.trim_end().replace('\n', "\r\n")).into_bytes()
    }

    fn report() -> ReplayReport {
        let peer = "127.0.0.1:5060".parse::<SocketAddr>().unwrap();
        let call_id = CallId::from_sequence(1);
        let scenario = Scenario::new(
            "inbound-cancelled-differential",
            vec![
                ScenarioStep::ReceiveSip {
                    at: Duration::ZERO,
                    source: peer,
                    reliability: TransportReliability::Unreliable,
                    wire: sip_fixture(include_str!("../tests/fixtures/invite.sip")),
                },
                ScenarioStep::NegotiateAudio {
                    call_id: call_id.clone(),
                    local_sdp: include_bytes!("../tests/fixtures/local.sdp").to_vec(),
                    remote_sdp: include_bytes!("../tests/fixtures/remote.sdp").to_vec(),
                },
                ScenarioStep::RespondToInvite {
                    at: Duration::from_millis(10),
                    call_id: call_id.clone(),
                    status_code: 180,
                    reason: "Ringing".to_owned(),
                    body: Vec::new(),
                },
                ScenarioStep::ReceiveSip {
                    at: Duration::from_millis(20),
                    source: peer,
                    reliability: TransportReliability::Unreliable,
                    wire: sip_fixture(include_str!("../tests/fixtures/cancel.sip")),
                },
                ScenarioStep::ReclaimTerminalCall { call_id },
            ],
        );
        ReplayRunner::new(
            ReplayConfig::default(),
            CallEngine::new(EngineConfig::default()).unwrap(),
        )
        .unwrap()
        .run(&scenario)
        .unwrap()
    }

    #[test]
    fn synthetic_rust_observation_matches_checked_in_oracle() {
        let actual = normalize_replay(&report(), NormalizationConfig::default()).unwrap();
        let expected = parse_oracle_fixture(ORACLE, FixtureConfig::default()).unwrap();
        let comparison = compare(&actual, &expected, ComparisonConfig::default()).unwrap();

        assert_eq!(comparison.differences, Vec::new());
        assert_eq!(comparison.total_differences, 0);
        assert!(comparison.matched);
    }

    #[test]
    fn normalization_removes_environment_owned_identifiers_addresses_and_timing() {
        let normalized = normalize_replay(&report(), NormalizationConfig::default()).unwrap();
        let fixture = normalized.to_fixture();

        for raw in [
            "fixture-diff-call",
            "fixture-diff-branch",
            "127.0.0.1",
            "192.0.2.10",
            "203.0.113.20",
            "rust-1",
            "10ms",
            "20ms",
        ] {
            assert!(!fixture.contains(raw), "raw value leaked: {raw}");
        }
        assert!(fixture.contains("timing order-only"));
        assert!(fixture.contains("response 100 cseq=1/INVITE"));
        assert!(fixture.contains("codec=PCMU/8000/1:fmtp=false"));
        assert!(fixture.contains("cleanup calls=0 bridges=0 transactions=0"));
    }

    #[test]
    fn comparison_reports_and_bounds_semantic_differences() {
        let actual = normalize_replay(&report(), NormalizationConfig::default()).unwrap();
        let changed = ORACLE
            .replace("response 180", "response 183")
            .replace("cleanup calls=0", "cleanup calls=1");
        let expected = parse_oracle_fixture(&changed, FixtureConfig::default()).unwrap();
        let comparison =
            compare(&actual, &expected, ComparisonConfig { max_differences: 1 }).unwrap();

        assert!(!comparison.matched);
        assert_eq!(comparison.total_differences, 2);
        assert_eq!(comparison.differences.len(), 1);
        assert!(
            comparison.differences[0]
                .expected
                .as_deref()
                .unwrap()
                .contains("response 183")
        );
    }

    #[test]
    fn fixture_parser_rejects_unknown_categories_and_bounds() {
        let unknown = "version\t1\nscenario\ttest\nfact\tunknown value\n";
        assert_eq!(
            parse_oracle_fixture(unknown, FixtureConfig::default()),
            Err(DifferentialError::InvalidFixture {
                line: 3,
                reason: "unknown fact category",
            })
        );
        assert_eq!(
            parse_oracle_fixture(
                ORACLE,
                FixtureConfig {
                    max_bytes: 1,
                    ..FixtureConfig::default()
                }
            ),
            Err(DifferentialError::FixtureTooLarge {
                actual: ORACLE.len(),
                maximum: 1,
            })
        );
        assert_eq!(
            compare(
                &normalize_replay(&report(), NormalizationConfig::default()).unwrap(),
                &parse_oracle_fixture(ORACLE, FixtureConfig::default()).unwrap(),
                ComparisonConfig { max_differences: 0 },
            ),
            Err(DifferentialError::InvalidConfig)
        );
    }
}
