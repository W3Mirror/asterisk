//! Safe parsing of the RTCP sender/receiver reports needed for observability.

use std::{
    error::Error,
    fmt::{Display, Formatter},
    net::SocketAddr,
    time::Duration,
};

use sip_security::SourceIpPolicy;

const MAX_PACKET_BYTES: usize = 65_535;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReceptionReport {
    pub source_ssrc: u32,
    pub fraction_lost: u8,
    pub cumulative_lost: i32,
    pub highest_sequence: u32,
    pub jitter: u32,
    pub last_sender_report: u32,
    pub delay_since_last_sender_report: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SenderReport {
    pub ssrc: u32,
    pub ntp_msw: u32,
    pub ntp_lsw: u32,
    pub rtp_timestamp: u32,
    pub packets_sent: u32,
    pub octets_sent: u32,
    pub reports: Vec<ReceptionReport>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReceiverReport {
    pub ssrc: u32,
    pub reports: Vec<ReceptionReport>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RtcpPacket {
    SenderReport(SenderReport),
    ReceiverReport(ReceiverReport),
    Unknown { packet_type: u8, body: Vec<u8> },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseError {
    TooShort,
    UnsupportedVersion(u8),
    InvalidLength,
    PacketTooLarge,
    InvalidReportCount,
    InvalidReport,
    InvalidPadding,
}

impl Display for ParseError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooShort => formatter.write_str("RTCP packet is shorter than its header"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported RTCP version {version}")
            }
            Self::InvalidLength => formatter.write_str("RTCP length exceeds packet bounds"),
            Self::PacketTooLarge => formatter.write_str("RTCP compound packet exceeds size limit"),
            Self::InvalidReportCount => formatter.write_str("RTCP report count is invalid"),
            Self::InvalidReport => formatter.write_str("RTCP reception report is invalid"),
            Self::InvalidPadding => formatter.write_str("RTCP padding is invalid"),
        }
    }
}

impl Error for ParseError {}

pub fn parse(input: &[u8]) -> Result<Vec<RtcpPacket>, ParseError> {
    if input.len() > MAX_PACKET_BYTES {
        return Err(ParseError::PacketTooLarge);
    }
    if input.is_empty() {
        return Err(ParseError::TooShort);
    }
    let mut offset = 0usize;
    let mut packets = Vec::new();
    while offset < input.len() {
        if input.len() - offset < 4 {
            return Err(ParseError::TooShort);
        }
        let length_words = usize::from(u16::from_be_bytes([input[offset + 2], input[offset + 3]]));
        let length = length_words
            .checked_add(1)
            .and_then(|words| words.checked_mul(4))
            .ok_or(ParseError::InvalidLength)?;
        let end = offset
            .checked_add(length)
            .ok_or(ParseError::InvalidLength)?;
        if end > input.len() || length < 4 {
            return Err(ParseError::InvalidLength);
        }
        packets.push(parse_one(&input[offset..end])?);
        offset = end;
    }
    Ok(packets)
}

/// Configuration for a bounded RTCP receive session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RtcpSessionConfig {
    /// Maximum compound RTCP datagram size accepted by the session.
    pub max_packet_bytes: usize,
    /// Optional expected remote SSRC for known sender/receiver reports.
    pub remote_ssrc: Option<u32>,
}

impl Default for RtcpSessionConfig {
    fn default() -> Self {
        Self {
            max_packet_bytes: MAX_PACKET_BYTES,
            remote_ssrc: None,
        }
    }
}

impl RtcpSessionConfig {
    fn validate(self) -> Result<Self, SessionError> {
        if !(4..=MAX_PACKET_BYTES).contains(&self.max_packet_bytes) {
            return Err(SessionError::InvalidPacketLimit);
        }
        Ok(self)
    }
}

/// Errors raised while validating or driving an RTCP receive session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionError {
    /// The configured RTCP datagram bound cannot contain an RTCP header.
    InvalidPacketLimit,
    /// An RTCP datagram exceeded the configured session bound.
    PacketTooLarge { actual: usize, maximum: usize },
    /// A known report used a different synchronization source than expected.
    UnexpectedSsrc { expected: u32, actual: u32 },
    /// The observed peer address was rejected before RTCP parsing.
    SourceAddressDenied { source: SocketAddr },
    /// The serialized RTCP input was malformed.
    Parse(ParseError),
    /// RTCP serialization failed.
    Serialize(SerializeError),
}

impl Display for SessionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPacketLimit => {
                formatter.write_str("RTCP session packet limit must include the fixed header")
            }
            Self::PacketTooLarge { actual, maximum } => {
                write!(
                    formatter,
                    "RTCP session packet is {actual} bytes, maximum is {maximum}"
                )
            }
            Self::UnexpectedSsrc { expected, actual } => {
                write!(
                    formatter,
                    "RTCP session expected SSRC {expected}, received {actual}"
                )
            }
            Self::SourceAddressDenied { source } => {
                write!(formatter, "RTCP source address {source} is not allowed")
            }
            Self::Parse(error) => Display::fmt(error, formatter),
            Self::Serialize(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for SessionError {}

impl From<ParseError> for SessionError {
    fn from(error: ParseError) -> Self {
        Self::Parse(error)
    }
}

impl From<SerializeError> for SessionError {
    fn from(error: SerializeError) -> Self {
        Self::Serialize(error)
    }
}

/// Aggregated RTCP send/receive counters and observed SSRC state.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RtcpSessionStats {
    /// Number of RTCP datagrams serialized by the session.
    pub packets_sent: u64,
    /// Number of serialized RTCP octets sent by the session.
    pub octets_sent: u64,
    /// Number of RTCP packets accepted from parsed compound datagrams.
    pub packets_received: u64,
    /// Number of serialized RTCP octets accepted.
    pub octets_received: u64,
    /// Most recently reported non-negative cumulative packet loss.
    pub packets_lost: u64,
    /// Most recently reported interarrival jitter, in RTP timestamp units.
    pub jitter: u32,
    /// Latest RTT estimate from a matching Sender Report and reception report.
    pub round_trip: Option<Duration>,
    /// Number of datagrams rejected after source authorization.
    pub invalid_packets: u64,
    /// Number of changes between known report SSRCs.
    pub ssrc_changes: u64,
    /// Arrival time of the most recently accepted datagram.
    pub last_received: Option<Duration>,
}

/// Bounded RTCP receive session with observed-source and SSRC validation.
#[derive(Clone, Debug)]
pub struct RtcpSession {
    config: RtcpSessionConfig,
    source_policy: SourceIpPolicy,
    remote_ssrc: Option<u32>,
    last_sender_report: Option<SenderReportObservation>,
    stats: RtcpSessionStats,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SenderReportObservation {
    lsr: u32,
    received_at: Duration,
}

/// Alias that makes the RTCP-specific session error name discoverable.
pub type RtcpSessionError = SessionError;

impl RtcpSession {
    /// Creates a session with the default-allow source policy.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::InvalidPacketLimit`] when the RTCP size bound
    /// cannot contain the fixed header.
    pub fn new(config: RtcpSessionConfig) -> Result<Self, SessionError> {
        Self::new_with_source_policy(config, SourceIpPolicy::default())
    }

    /// Creates a session with an explicit observed-source policy.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::InvalidPacketLimit`] when the RTCP size bound
    /// cannot contain the fixed header.
    pub fn new_with_source_policy(
        config: RtcpSessionConfig,
        source_policy: SourceIpPolicy,
    ) -> Result<Self, SessionError> {
        let config = config.validate()?;
        Ok(Self {
            remote_ssrc: config.remote_ssrc,
            config,
            source_policy,
            last_sender_report: None,
            stats: RtcpSessionStats::default(),
        })
    }

    /// Replaces the observed-source policy while preserving session state.
    #[must_use]
    pub fn with_source_policy(mut self, source_policy: SourceIpPolicy) -> Self {
        self.source_policy = source_policy;
        self
    }

    /// Borrows the configured RTCP bounds.
    #[must_use]
    pub const fn config(&self) -> RtcpSessionConfig {
        self.config
    }

    /// Borrows the observed-source policy applied by source-aware receives.
    #[must_use]
    pub fn source_policy(&self) -> &SourceIpPolicy {
        &self.source_policy
    }

    /// Checks an observed source address before RTCP parsing or state changes.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::SourceAddressDenied`] when the source policy
    /// rejects `source`.
    pub fn authorize_source(&self, source: SocketAddr) -> Result<(), SessionError> {
        if self.source_policy.allows_socket(source) {
            Ok(())
        } else {
            Err(SessionError::SourceAddressDenied { source })
        }
    }

    /// Parses and validates one RTCP datagram, updating receive metrics.
    ///
    /// # Errors
    ///
    /// Returns a size, parse, or expected-SSRC error. Invalid datagrams are
    /// counted, while successful parsing updates packet and octet counters.
    pub fn receive(
        &mut self,
        input: &[u8],
        arrival: Duration,
    ) -> Result<Vec<RtcpPacket>, SessionError> {
        if input.len() > self.config.max_packet_bytes {
            self.stats.invalid_packets = self.stats.invalid_packets.saturating_add(1);
            return Err(SessionError::PacketTooLarge {
                actual: input.len(),
                maximum: self.config.max_packet_bytes,
            });
        }
        let packets = parse(input).map_err(|error| {
            self.stats.invalid_packets = self.stats.invalid_packets.saturating_add(1);
            SessionError::Parse(error)
        })?;
        if let Some(expected) = self.config.remote_ssrc {
            for actual in packets.iter().filter_map(packet_ssrc) {
                if actual != expected {
                    self.stats.invalid_packets = self.stats.invalid_packets.saturating_add(1);
                    return Err(SessionError::UnexpectedSsrc { expected, actual });
                }
            }
        }
        for actual in packets.iter().filter_map(packet_ssrc) {
            if self.remote_ssrc.is_some_and(|previous| previous != actual) {
                self.stats.ssrc_changes = self.stats.ssrc_changes.saturating_add(1);
            }
            self.remote_ssrc = Some(actual);
        }
        self.observe_quality(&packets, arrival);
        self.stats.packets_received = self
            .stats
            .packets_received
            .saturating_add(packets.len() as u64);
        self.stats.octets_received = self
            .stats
            .octets_received
            .saturating_add(input.len() as u64);
        self.stats.last_received = Some(arrival);
        Ok(packets)
    }

    /// Parses and validates one RTCP datagram from an observed peer.
    ///
    /// Source authorization runs before size checks and parsing, so a denied
    /// peer cannot change parse counters, SSRC state, or receive metrics.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::SourceAddressDenied`] before parsing when the
    /// source is rejected, or forwards the usual RTCP session errors.
    pub fn receive_from(
        &mut self,
        input: &[u8],
        source: SocketAddr,
        arrival: Duration,
    ) -> Result<Vec<RtcpPacket>, SessionError> {
        self.authorize_source(source)?;
        self.receive(input, arrival)
    }

    /// Serializes one RTCP packet and updates send metrics.
    ///
    /// # Errors
    ///
    /// Returns a serialization error or [`SessionError::PacketTooLarge`] when
    /// the encoded datagram exceeds the configured session bound. A failed
    /// send does not advance the send counters.
    pub fn send(&mut self, packet: &RtcpPacket) -> Result<Vec<u8>, SessionError> {
        let wire = serialize(packet)?;
        if wire.len() > self.config.max_packet_bytes {
            return Err(SessionError::PacketTooLarge {
                actual: wire.len(),
                maximum: self.config.max_packet_bytes,
            });
        }
        self.stats.packets_sent = self.stats.packets_sent.saturating_add(1);
        self.stats.octets_sent = self.stats.octets_sent.saturating_add(wire.len() as u64);
        Ok(wire)
    }

    /// Returns a snapshot of RTCP send and receive metrics.
    #[must_use]
    pub fn stats(&self) -> RtcpSessionStats {
        self.stats.clone()
    }

    /// Returns the LSR and DLSR fields for a reception report generated at `now`.
    ///
    /// Both values are zero until a non-zero Sender Report timestamp has been
    /// accepted. DLSR uses the RTCP 16.16-second representation and saturates
    /// when the elapsed monotonic duration exceeds that field's range.
    #[must_use]
    pub fn reception_report_timing(&self, now: Duration) -> (u32, u32) {
        let Some(sender) = self.last_sender_report else {
            return (0, 0);
        };
        (
            sender.lsr,
            duration_to_ntp_short(now.saturating_sub(sender.received_at)),
        )
    }

    /// Returns the most recently observed known report SSRC.
    #[must_use]
    pub const fn remote_ssrc(&self) -> Option<u32> {
        self.remote_ssrc
    }

    fn observe_quality(&mut self, packets: &[RtcpPacket], arrival: Duration) {
        for packet in packets {
            match packet {
                RtcpPacket::SenderReport(report) => {
                    self.observe_reports(&report.reports, arrival);
                    let lsr = ntp_middle_32(report.ntp_msw, report.ntp_lsw);
                    if lsr != 0 {
                        self.last_sender_report = Some(SenderReportObservation {
                            lsr,
                            received_at: arrival,
                        });
                    }
                }
                RtcpPacket::ReceiverReport(report) => {
                    self.observe_reports(&report.reports, arrival);
                }
                RtcpPacket::Unknown { .. } => {}
            }
        }
    }

    fn observe_reports(&mut self, reports: &[ReceptionReport], arrival: Duration) {
        for report in reports {
            self.stats.packets_lost = u64::try_from(report.cumulative_lost.max(0)).unwrap_or(0);
            self.stats.jitter = report.jitter;
            if report.last_sender_report == 0 {
                continue;
            }
            let Some(sender) = self.last_sender_report else {
                continue;
            };
            if sender.lsr != report.last_sender_report {
                continue;
            }
            let elapsed = arrival.saturating_sub(sender.received_at);
            let Some(round_trip) =
                elapsed.checked_sub(ntp_short_to_duration(report.delay_since_last_sender_report))
            else {
                continue;
            };
            self.stats.round_trip = Some(round_trip);
        }
    }
}

fn ntp_middle_32(most_significant_word: u32, least_significant_word: u32) -> u32 {
    (most_significant_word << 16) | (least_significant_word >> 16)
}

fn ntp_short_to_duration(value: u32) -> Duration {
    let seconds = u64::from(value >> 16);
    let nanos = (u64::from(value & 0xffff) * 1_000_000_000) / 65_536;
    Duration::from_secs(seconds).saturating_add(Duration::from_nanos(nanos))
}

fn duration_to_ntp_short(value: Duration) -> u32 {
    if value.as_secs() > u64::from(u16::MAX) {
        return u32::MAX;
    }
    let seconds = value.as_secs();
    let fraction = (u64::from(value.subsec_nanos()) * 65_536) / 1_000_000_000;
    u32::try_from((seconds << 16) | fraction).unwrap_or(u32::MAX)
}

fn packet_ssrc(packet: &RtcpPacket) -> Option<u32> {
    match packet {
        RtcpPacket::SenderReport(report) => Some(report.ssrc),
        RtcpPacket::ReceiverReport(report) => Some(report.ssrc),
        RtcpPacket::Unknown { .. } => None,
    }
}

fn parse_one(input: &[u8]) -> Result<RtcpPacket, ParseError> {
    let first = input[0];
    if first >> 6 != 2 {
        return Err(ParseError::UnsupportedVersion(first >> 6));
    }
    let report_count = usize::from(first & 0x1f);
    let packet_type = input[1];
    let mut body = &input[4..];
    if first & 0x20 != 0 {
        let padding = usize::from(*body.last().ok_or(ParseError::InvalidPadding)?);
        if padding == 0 || padding > body.len() {
            return Err(ParseError::InvalidPadding);
        }
        body = &body[..body.len() - padding];
    }
    match packet_type {
        200 => {
            let fixed = 24usize;
            let reports_bytes = report_count
                .checked_mul(24)
                .ok_or(ParseError::InvalidReportCount)?;
            if body.len() < fixed || body.len() != fixed + reports_bytes {
                return Err(ParseError::InvalidReport);
            }
            let sender = SenderReport {
                ssrc: read_u32(body, 0)?,
                ntp_msw: read_u32(body, 4)?,
                ntp_lsw: read_u32(body, 8)?,
                rtp_timestamp: read_u32(body, 12)?,
                packets_sent: read_u32(body, 16)?,
                octets_sent: read_u32(body, 20)?,
                reports: parse_reports(&body[fixed..], report_count)?,
            };
            Ok(RtcpPacket::SenderReport(sender))
        }
        201 => {
            let reports_bytes = report_count
                .checked_mul(24)
                .ok_or(ParseError::InvalidReportCount)?;
            if body.len() < 4 || body.len() != 4 + reports_bytes {
                return Err(ParseError::InvalidReport);
            }
            Ok(RtcpPacket::ReceiverReport(ReceiverReport {
                ssrc: read_u32(body, 0)?,
                reports: parse_reports(&body[4..], report_count)?,
            }))
        }
        _ => Ok(RtcpPacket::Unknown {
            packet_type,
            body: body.to_vec(),
        }),
    }
}

fn parse_reports(input: &[u8], count: usize) -> Result<Vec<ReceptionReport>, ParseError> {
    let expected = count
        .checked_mul(24)
        .ok_or(ParseError::InvalidReportCount)?;
    if input.len() != expected {
        return Err(ParseError::InvalidReport);
    }
    input
        .chunks_exact(24)
        .map(|report| {
            let cumulative =
                ((i32::from(report[5]) << 16) | (i32::from(report[6]) << 8) | i32::from(report[7]))
                    .wrapping_sub(if report[5] & 0x80 != 0 { 1 << 24 } else { 0 });
            Ok(ReceptionReport {
                source_ssrc: read_u32(report, 0)?,
                fraction_lost: report[4],
                cumulative_lost: cumulative,
                highest_sequence: read_u32(report, 8)?,
                jitter: read_u32(report, 12)?,
                last_sender_report: read_u32(report, 16)?,
                delay_since_last_sender_report: read_u32(report, 20)?,
            })
        })
        .collect()
}

fn read_u32(input: &[u8], offset: usize) -> Result<u32, ParseError> {
    let bytes = input
        .get(offset..offset + 4)
        .ok_or(ParseError::InvalidLength)?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

pub fn serialize(packet: &RtcpPacket) -> Result<Vec<u8>, SerializeError> {
    let (packet_type, report_count, mut body) = match packet {
        RtcpPacket::SenderReport(sender) => {
            if sender.reports.len() > 31 {
                return Err(SerializeError::TooManyReports);
            }
            let mut body = Vec::with_capacity(24 + sender.reports.len() * 24);
            for value in [
                sender.ssrc,
                sender.ntp_msw,
                sender.ntp_lsw,
                sender.rtp_timestamp,
                sender.packets_sent,
                sender.octets_sent,
            ] {
                body.extend_from_slice(&value.to_be_bytes());
            }
            for report in &sender.reports {
                append_report(&mut body, report)?;
            }
            (200, sender.reports.len(), body)
        }
        RtcpPacket::ReceiverReport(receiver) => {
            if receiver.reports.len() > 31 {
                return Err(SerializeError::TooManyReports);
            }
            let mut body = Vec::with_capacity(4 + receiver.reports.len() * 24);
            body.extend_from_slice(&receiver.ssrc.to_be_bytes());
            for report in &receiver.reports {
                append_report(&mut body, report)?;
            }
            (201, receiver.reports.len(), body)
        }
        RtcpPacket::Unknown { packet_type, body } => (u32::from(*packet_type), 0, body.clone()),
    };
    if body.len() % 4 != 0 || body.len() / 4 > usize::from(u16::MAX) {
        return Err(SerializeError::InvalidBody);
    }
    let mut output = Vec::with_capacity(body.len() + 4);
    output.push((2 << 6) | report_count as u8);
    output.push(packet_type as u8);
    output.extend_from_slice(
        &u16::try_from(body.len() / 4)
            .unwrap_or(u16::MAX)
            .to_be_bytes(),
    );
    output.append(&mut body);
    Ok(output)
}

fn append_report(output: &mut Vec<u8>, report: &ReceptionReport) -> Result<(), SerializeError> {
    if !(-0x80_0000..=0x7f_ffff).contains(&report.cumulative_lost) {
        return Err(SerializeError::InvalidLoss);
    }
    output.extend_from_slice(&report.source_ssrc.to_be_bytes());
    output.push(report.fraction_lost);
    let loss = (report.cumulative_lost as i64 & 0x00ff_ffff) as u32;
    output.extend_from_slice(&loss.to_be_bytes()[1..]);
    output.extend_from_slice(&report.highest_sequence.to_be_bytes());
    output.extend_from_slice(&report.jitter.to_be_bytes());
    output.extend_from_slice(&report.last_sender_report.to_be_bytes());
    output.extend_from_slice(&report.delay_since_last_sender_report.to_be_bytes());
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SerializeError {
    TooManyReports,
    InvalidLoss,
    InvalidBody,
}

impl Display for SerializeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManyReports => formatter.write_str("RTCP supports at most 31 report blocks"),
            Self::InvalidLoss => {
                formatter.write_str("RTCP cumulative loss does not fit in 24 bits")
            }
            Self::InvalidBody => formatter.write_str("RTCP body is not a whole number of words"),
        }
    }
}

impl Error for SerializeError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> ReceptionReport {
        ReceptionReport {
            source_ssrc: 9,
            fraction_lost: 1,
            cumulative_lost: -2,
            highest_sequence: 100,
            jitter: 4,
            last_sender_report: 5,
            delay_since_last_sender_report: 6,
        }
    }

    #[test]
    fn sender_and_receiver_reports_round_trip() {
        let sender = RtcpPacket::SenderReport(SenderReport {
            ssrc: 42,
            ntp_msw: 1,
            ntp_lsw: 2,
            rtp_timestamp: 3,
            packets_sent: 4,
            octets_sent: 5,
            reports: vec![report()],
        });
        assert_eq!(parse(&serialize(&sender).unwrap()).unwrap(), vec![sender]);
        let receiver = RtcpPacket::ReceiverReport(ReceiverReport {
            ssrc: 42,
            reports: vec![report()],
        });
        assert_eq!(
            parse(&serialize(&receiver).unwrap()).unwrap(),
            vec![receiver]
        );
    }

    #[test]
    fn malformed_compound_packets_do_not_panic() {
        for length in 0..256 {
            let input = (0..length)
                .map(|offset| (offset as u8).wrapping_mul(17))
                .collect::<Vec<_>>();
            let _ = parse(&input);
        }
    }

    #[test]
    fn parses_valid_padding_and_rejects_invalid_padding() {
        // A receiver report with four bytes of RTCP padding. The padding is
        // outside the report body and must not change its decoded fields.
        let mut wire = serialize(&RtcpPacket::ReceiverReport(ReceiverReport {
            ssrc: 42,
            reports: Vec::new(),
        }))
        .unwrap();
        wire[0] |= 0x20;
        wire[2..4].copy_from_slice(&2u16.to_be_bytes());
        wire.extend_from_slice(&[0, 0, 0, 4]);
        let parsed = parse(&wire).unwrap();
        assert_eq!(
            parsed,
            vec![RtcpPacket::ReceiverReport(ReceiverReport {
                ssrc: 42,
                reports: Vec::new(),
            })]
        );

        let mut invalid = wire;
        *invalid.last_mut().unwrap() = 0;
        assert!(matches!(parse(&invalid), Err(ParseError::InvalidPadding)));
    }

    #[test]
    fn preserves_signed_24_bit_loss_boundaries() {
        for cumulative_lost in [-0x80_0000, -1, 0, 0x7f_ffff] {
            let packet = RtcpPacket::ReceiverReport(ReceiverReport {
                ssrc: 1,
                reports: vec![ReceptionReport {
                    cumulative_lost,
                    ..report()
                }],
            });
            assert_eq!(parse(&serialize(&packet).unwrap()).unwrap(), vec![packet]);
        }
        let invalid = RtcpPacket::ReceiverReport(ReceiverReport {
            ssrc: 1,
            reports: vec![ReceptionReport {
                cumulative_lost: 0x80_0000,
                ..report()
            }],
        });
        assert!(matches!(
            serialize(&invalid),
            Err(SerializeError::InvalidLoss)
        ));
    }

    #[test]
    fn session_tracks_receive_metrics_and_ssrc_changes() {
        let mut session = RtcpSession::new(RtcpSessionConfig::default()).unwrap();
        let first_packet = RtcpPacket::ReceiverReport(ReceiverReport {
            ssrc: 42,
            reports: Vec::new(),
        });
        let first = session.send(&first_packet).unwrap();
        session.receive(&first, Duration::from_millis(1)).unwrap();
        let second_packet = RtcpPacket::ReceiverReport(ReceiverReport {
            ssrc: 43,
            reports: Vec::new(),
        });
        let second = session.send(&second_packet).unwrap();
        session.receive(&second, Duration::from_millis(2)).unwrap();

        let stats = session.stats();
        assert_eq!(stats.packets_sent, 2);
        assert_eq!(stats.octets_sent, (first.len() + second.len()) as u64);
        assert_eq!(stats.packets_received, 2);
        assert_eq!(stats.octets_received, (first.len() + second.len()) as u64);
        assert_eq!(stats.ssrc_changes, 1);
        assert_eq!(stats.last_received, Some(Duration::from_millis(2)));
        assert_eq!(session.remote_ssrc(), Some(43));
    }

    #[test]
    fn session_tracks_report_quality_and_matching_round_trip() {
        let mut session = RtcpSession::new(RtcpSessionConfig::default()).unwrap();
        let sender = RtcpPacket::SenderReport(SenderReport {
            ssrc: 42,
            ntp_msw: 1,
            ntp_lsw: 0,
            rtp_timestamp: 0,
            packets_sent: 10,
            octets_sent: 80,
            reports: Vec::new(),
        });
        let sender_wire = serialize(&sender).unwrap();
        session
            .receive(&sender_wire, Duration::from_secs(10))
            .unwrap();
        assert_eq!(
            session.reception_report_timing(Duration::from_secs(12)),
            (ntp_middle_32(1, 0), 2 << 16)
        );

        let receiver = RtcpPacket::ReceiverReport(ReceiverReport {
            ssrc: 42,
            reports: vec![ReceptionReport {
                cumulative_lost: 12,
                jitter: 320,
                last_sender_report: ntp_middle_32(1, 0),
                delay_since_last_sender_report: 0x0000_8000,
                ..report()
            }],
        });
        let receiver_wire = serialize(&receiver).unwrap();
        session
            .receive(&receiver_wire, Duration::from_secs(12))
            .unwrap();

        let stats = session.stats();
        assert_eq!(stats.packets_lost, 12);
        assert_eq!(stats.jitter, 320);
        assert_eq!(stats.round_trip, Some(Duration::from_millis(1_500)));
    }

    #[test]
    fn reception_report_timing_is_zero_without_a_sender_report_and_saturates() {
        let mut session = RtcpSession::new(RtcpSessionConfig::default()).unwrap();
        assert_eq!(session.reception_report_timing(Duration::MAX), (0, 0));
        let sender = RtcpPacket::SenderReport(SenderReport {
            ssrc: 42,
            ntp_msw: 1,
            ntp_lsw: 0,
            rtp_timestamp: 0,
            packets_sent: 0,
            octets_sent: 0,
            reports: Vec::new(),
        });
        session
            .receive(&serialize(&sender).unwrap(), Duration::ZERO)
            .unwrap();
        assert_eq!(
            session.reception_report_timing(Duration::from_secs(65_536)),
            (ntp_middle_32(1, 0), u32::MAX)
        );
    }

    #[test]
    fn negative_reported_loss_is_clamped_and_jitter_is_retained() {
        let mut session = RtcpSession::new(RtcpSessionConfig::default()).unwrap();
        let receiver = RtcpPacket::ReceiverReport(ReceiverReport {
            ssrc: 42,
            reports: vec![ReceptionReport {
                cumulative_lost: -2,
                jitter: 9,
                ..report()
            }],
        });
        let wire = serialize(&receiver).unwrap();
        session.receive(&wire, Duration::ZERO).unwrap();

        let stats = session.stats();
        assert_eq!(stats.packets_lost, 0);
        assert_eq!(stats.jitter, 9);
        assert_eq!(stats.round_trip, None);
    }

    #[test]
    fn source_policy_rejects_before_parse_and_state_mutation() {
        let mut policy = SourceIpPolicy::default();
        policy.add_allow("198.51.100.0/24").unwrap();
        policy.add_deny("198.51.100.128/25").unwrap();
        let mut session =
            RtcpSession::new_with_source_policy(RtcpSessionConfig::default(), policy).unwrap();
        let baseline = session.stats();
        let denied = "198.51.100.200:5000".parse().unwrap();
        assert_eq!(
            session.receive_from(&[], denied, Duration::ZERO),
            Err(SessionError::SourceAddressDenied { source: denied })
        );
        assert_eq!(session.stats(), baseline);
        assert_eq!(session.remote_ssrc(), None);

        let wire = serialize(&RtcpPacket::ReceiverReport(ReceiverReport {
            ssrc: 42,
            reports: Vec::new(),
        }))
        .unwrap();
        let allowed = "198.51.100.10:5000".parse().unwrap();
        session
            .receive_from(&wire, allowed, Duration::from_millis(1))
            .unwrap();
        assert_eq!(session.stats().packets_received, 1);
        assert_eq!(session.remote_ssrc(), Some(42));
    }

    #[test]
    fn source_policy_keeps_ipv4_and_ipv6_families_separate() {
        let mut policy = SourceIpPolicy::default();
        policy.add_allow("2001:db8::/32").unwrap();
        let mut session =
            RtcpSession::new_with_source_policy(RtcpSessionConfig::default(), policy).unwrap();
        let wire = serialize(&RtcpPacket::ReceiverReport(ReceiverReport {
            ssrc: 7,
            reports: Vec::new(),
        }))
        .unwrap();
        let ipv6 = "[2001:db8::10]:5000".parse().unwrap();
        session.receive_from(&wire, ipv6, Duration::ZERO).unwrap();
        let before_ipv4 = session.stats();
        let ipv4 = "192.0.2.10:5000".parse().unwrap();
        assert_eq!(
            session.receive_from(&wire, ipv4, Duration::from_millis(1)),
            Err(SessionError::SourceAddressDenied { source: ipv4 })
        );
        assert_eq!(session.stats(), before_ipv4);
    }

    #[test]
    fn session_rejects_invalid_bounds_and_unexpected_ssrc() {
        assert!(matches!(
            RtcpSession::new(RtcpSessionConfig {
                max_packet_bytes: 3,
                ..RtcpSessionConfig::default()
            }),
            Err(SessionError::InvalidPacketLimit)
        ));
        let mut limited = RtcpSession::new(RtcpSessionConfig {
            max_packet_bytes: 12,
            ..RtcpSessionConfig::default()
        })
        .unwrap();
        assert!(matches!(
            limited.receive(&[0; 13], Duration::ZERO),
            Err(SessionError::PacketTooLarge {
                actual: 13,
                maximum: 12
            })
        ));
        assert_eq!(limited.stats().invalid_packets, 1);

        let oversized = RtcpPacket::ReceiverReport(ReceiverReport {
            ssrc: 7,
            reports: vec![report()],
        });
        let oversized_len = serialize(&oversized).unwrap().len();
        assert!(matches!(
            limited.send(&oversized),
            Err(SessionError::PacketTooLarge {
                actual,
                maximum: 12
            }) if actual == oversized_len
        ));
        assert_eq!(limited.stats().packets_sent, 0);
        assert_eq!(limited.stats().octets_sent, 0);

        let mut expected = RtcpSession::new(RtcpSessionConfig {
            remote_ssrc: Some(42),
            ..RtcpSessionConfig::default()
        })
        .unwrap();
        let wire = serialize(&RtcpPacket::ReceiverReport(ReceiverReport {
            ssrc: 7,
            reports: Vec::new(),
        }))
        .unwrap();
        assert!(matches!(
            expected.receive(&wire, Duration::ZERO),
            Err(SessionError::UnexpectedSsrc {
                expected: 42,
                actual: 7
            })
        ));
        assert_eq!(expected.stats().invalid_packets, 1);
        assert_eq!(expected.remote_ssrc(), Some(42));
    }
}
