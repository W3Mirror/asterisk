//! Safe RTP packet parsing, serialization, and session statistics.

use std::{
    error::Error,
    fmt::{Display, Formatter},
    net::SocketAddr,
    time::Duration,
};

use sip_security::SourceIpPolicy;

#[derive(Clone, Copy, Debug)]
pub struct ParseConfig {
    pub max_packet_bytes: usize,
    pub max_extension_bytes: usize,
}

impl Default for ParseConfig {
    fn default() -> Self {
        Self {
            max_packet_bytes: 65_535,
            max_extension_bytes: 4_096,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RtpExtension {
    pub profile: u16,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RtpPacket {
    pub padding: bool,
    pub marker: bool,
    pub payload_type: u8,
    pub sequence_number: u16,
    pub timestamp: u32,
    pub ssrc: u32,
    pub csrcs: Vec<u32>,
    pub extension: Option<RtpExtension>,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseError {
    PacketTooLarge { actual: usize, maximum: usize },
    TooShort,
    UnsupportedVersion(u8),
    InvalidCsrcCount,
    InvalidExtension,
    ExtensionTooLarge { actual: usize, maximum: usize },
    InvalidPadding,
}

impl Display for ParseError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PacketTooLarge { actual, maximum } => {
                write!(
                    formatter,
                    "RTP packet is {actual} bytes, maximum is {maximum}"
                )
            }
            Self::TooShort => formatter.write_str("RTP packet is shorter than its fixed header"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported RTP version {version}")
            }
            Self::InvalidCsrcCount => formatter.write_str("RTP CSRC list exceeds packet bounds"),
            Self::InvalidExtension => {
                formatter.write_str("RTP header extension exceeds packet bounds")
            }
            Self::ExtensionTooLarge { actual, maximum } => {
                write!(
                    formatter,
                    "RTP extension is {actual} bytes, maximum is {maximum}"
                )
            }
            Self::InvalidPadding => formatter.write_str("RTP padding is invalid"),
        }
    }
}

impl Error for ParseError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SerializeError {
    TooManyCsrcs,
    InvalidPayloadType,
    ExtensionNotWordAligned,
    ExtensionTooLarge,
}

impl Display for SerializeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManyCsrcs => formatter.write_str("RTP supports at most 15 CSRC entries"),
            Self::InvalidPayloadType => {
                formatter.write_str("RTP payload type must fit in seven bits")
            }
            Self::ExtensionNotWordAligned => {
                formatter.write_str("RTP extension data must be a multiple of four bytes")
            }
            Self::ExtensionTooLarge => formatter.write_str("RTP extension is too large"),
        }
    }
}

impl Error for SerializeError {}

pub fn parse(input: &[u8]) -> Result<RtpPacket, ParseError> {
    parse_with_config(input, ParseConfig::default())
}

pub fn parse_with_config(input: &[u8], config: ParseConfig) -> Result<RtpPacket, ParseError> {
    if input.len() > config.max_packet_bytes {
        return Err(ParseError::PacketTooLarge {
            actual: input.len(),
            maximum: config.max_packet_bytes,
        });
    }
    if input.len() < 12 {
        return Err(ParseError::TooShort);
    }
    let first = input[0];
    let version = first >> 6;
    if version != 2 {
        return Err(ParseError::UnsupportedVersion(version));
    }
    let padding = first & 0x20 != 0;
    let has_extension = first & 0x10 != 0;
    let csrc_count = (first & 0x0f) as usize;
    let second = input[1];
    let marker = second & 0x80 != 0;
    let payload_type = second & 0x7f;
    let mut cursor = 12usize
        .checked_add(
            csrc_count
                .checked_mul(4)
                .ok_or(ParseError::InvalidCsrcCount)?,
        )
        .ok_or(ParseError::InvalidCsrcCount)?;
    if cursor > input.len() {
        return Err(ParseError::InvalidCsrcCount);
    }

    let mut csrcs = Vec::with_capacity(csrc_count);
    for chunk in input[12..cursor].chunks_exact(4) {
        csrcs.push(u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }

    let extension = if has_extension {
        if input.len().saturating_sub(cursor) < 4 {
            return Err(ParseError::InvalidExtension);
        }
        let profile = u16::from_be_bytes([input[cursor], input[cursor + 1]]);
        let length_words = usize::from(u16::from_be_bytes([input[cursor + 2], input[cursor + 3]]));
        let length = length_words
            .checked_mul(4)
            .ok_or(ParseError::InvalidExtension)?;
        if length > config.max_extension_bytes {
            return Err(ParseError::ExtensionTooLarge {
                actual: length,
                maximum: config.max_extension_bytes,
            });
        }
        let data_start = cursor + 4;
        let data_end = data_start
            .checked_add(length)
            .ok_or(ParseError::InvalidExtension)?;
        if data_end > input.len() {
            return Err(ParseError::InvalidExtension);
        }
        cursor = data_end;
        Some(RtpExtension {
            profile,
            data: input[data_start..data_end].to_vec(),
        })
    } else {
        None
    };

    let mut payload_end = input.len();
    if padding {
        let padding_bytes = usize::from(*input.last().ok_or(ParseError::InvalidPadding)?);
        if padding_bytes == 0 || padding_bytes > payload_end.saturating_sub(cursor) {
            return Err(ParseError::InvalidPadding);
        }
        payload_end -= padding_bytes;
    }
    if payload_end < cursor {
        return Err(ParseError::InvalidPadding);
    }
    Ok(RtpPacket {
        padding,
        marker,
        payload_type,
        sequence_number: u16::from_be_bytes([input[2], input[3]]),
        timestamp: u32::from_be_bytes([input[4], input[5], input[6], input[7]]),
        ssrc: u32::from_be_bytes([input[8], input[9], input[10], input[11]]),
        csrcs,
        extension,
        payload: input[cursor..payload_end].to_vec(),
    })
}

pub fn serialize(packet: &RtpPacket) -> Result<Vec<u8>, SerializeError> {
    if packet.csrcs.len() > 15 {
        return Err(SerializeError::TooManyCsrcs);
    }
    if packet.payload_type > 127 {
        return Err(SerializeError::InvalidPayloadType);
    }
    if let Some(extension) = &packet.extension {
        if extension.data.len() % 4 != 0 {
            return Err(SerializeError::ExtensionNotWordAligned);
        }
        if extension.data.len() / 4 > usize::from(u16::MAX) {
            return Err(SerializeError::ExtensionTooLarge);
        }
    }
    let has_extension = packet.extension.is_some();
    let extension_bytes = packet
        .extension
        .as_ref()
        .map_or(0, |extension| 4 + extension.data.len());
    let mut output = Vec::with_capacity(
        12 + packet.csrcs.len() * 4 + extension_bytes + packet.payload.len() + 1,
    );
    let mut first = 2 << 6;
    if packet.padding {
        first |= 0x20;
    }
    if has_extension {
        first |= 0x10;
    }
    first |= packet.csrcs.len() as u8;
    output.push(first);
    output.push((u8::from(packet.marker) << 7) | (packet.payload_type & 0x7f));
    output.extend_from_slice(&packet.sequence_number.to_be_bytes());
    output.extend_from_slice(&packet.timestamp.to_be_bytes());
    output.extend_from_slice(&packet.ssrc.to_be_bytes());
    for csrc in &packet.csrcs {
        output.extend_from_slice(&csrc.to_be_bytes());
    }
    if let Some(extension) = &packet.extension {
        output.extend_from_slice(&extension.profile.to_be_bytes());
        output.extend_from_slice(
            &(u16::try_from(extension.data.len() / 4).unwrap_or(u16::MAX)).to_be_bytes(),
        );
        output.extend_from_slice(&extension.data);
    }
    output.extend_from_slice(&packet.payload);
    if packet.padding {
        output.push(1);
    }
    Ok(output)
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RtpStats {
    pub packets_received: u64,
    pub packets_lost: u64,
    pub invalid_packets: u64,
    pub jitter: u32,
    pub ssrc_changes: u64,
    highest_extended_sequence: Option<i64>,
    last_transit: Option<i64>,
    ssrc: Option<u32>,
}

impl RtpStats {
    pub fn observe(
        &mut self,
        packet: &RtpPacket,
        arrival: Duration,
        clock_rate: u32,
    ) -> Result<(), StatsError> {
        if clock_rate == 0 {
            return Err(StatsError::InvalidClockRate);
        }
        self.packets_received = self.packets_received.saturating_add(1);
        if self
            .ssrc
            .is_some_and(|previous_ssrc| previous_ssrc != packet.ssrc)
        {
            self.ssrc_changes = self.ssrc_changes.saturating_add(1);
            // Sequence and transit state are scoped to an SSRC. Keeping it
            // across a source switch would manufacture loss and jitter from
            // unrelated streams.
            self.highest_extended_sequence = None;
            self.last_transit = None;
            self.jitter = 0;
        }
        self.ssrc = Some(packet.ssrc);

        let extended = self.extend_sequence(packet.sequence_number);
        if let Some(highest) = self.highest_extended_sequence {
            if extended > highest {
                self.packets_lost = self
                    .packets_lost
                    .saturating_add((extended - highest - 1).max(0) as u64);
                self.highest_extended_sequence = Some(extended);
            }
        } else {
            self.highest_extended_sequence = Some(extended);
        }

        let arrival_units = (arrival.as_nanos().saturating_mul(u128::from(clock_rate))
            / 1_000_000_000)
            .min(i64::MAX as u128) as i64;
        let transit = arrival_units - i64::from(packet.timestamp);
        if let Some(previous) = self.last_transit {
            let difference = (transit - previous).unsigned_abs().min(u64::from(u32::MAX)) as u32;
            self.jitter = self
                .jitter
                .saturating_add(difference.saturating_sub(self.jitter) / 16);
        }
        self.last_transit = Some(transit);
        Ok(())
    }

    fn extend_sequence(&self, sequence: u16) -> i64 {
        let Some(highest) = self.highest_extended_sequence else {
            return i64::from(sequence);
        };
        let highest_low = (highest & 0xffff) as u16;
        let mut extended = (highest & !0xffff) | i64::from(sequence);
        if sequence < highest_low && highest_low.wrapping_sub(sequence) > 0x8000 {
            extended += 0x1_0000;
        } else if sequence > highest_low && sequence.wrapping_sub(highest_low) > 0x8000 {
            extended -= 0x1_0000;
        }
        extended
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatsError {
    InvalidClockRate,
}

impl Display for StatsError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RTP clock rate must be non-zero")
    }
}

impl Error for StatsError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PayloadCodec {
    Pcmu,
    Pcma,
    G722,
    Opus,
    TelephoneEvent,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PayloadTypeMap([Option<PayloadCodec>; 128]);

impl Default for PayloadTypeMap {
    fn default() -> Self {
        Self(std::array::from_fn(|_| None))
    }
}

impl PayloadTypeMap {
    pub fn insert(&mut self, payload_type: u8, codec: PayloadCodec) {
        if let Some(slot) = self.0.get_mut(usize::from(payload_type)) {
            *slot = Some(codec);
        }
    }

    pub fn get(&self, payload_type: u8) -> Option<&PayloadCodec> {
        self.0
            .get(usize::from(payload_type))
            .and_then(Option::as_ref)
    }
}

/// Configuration for a bidirectional RTP session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RtpSessionConfig {
    /// Payload type used for packets sent and accepted by this session.
    pub payload_type: u8,
    /// RTP clock rate used for receive-side jitter accounting.
    pub clock_rate: u32,
    /// Maximum serialized RTP packet size accepted by the session.
    pub max_packet_bytes: usize,
    /// Maximum RTP header-extension payload accepted by the session.
    pub max_extension_bytes: usize,
    /// Local synchronization source identifier.
    pub local_ssrc: u32,
    /// Optional expected remote synchronization source identifier.
    pub remote_ssrc: Option<u32>,
}

impl Default for RtpSessionConfig {
    fn default() -> Self {
        Self {
            payload_type: 0,
            clock_rate: 8_000,
            max_packet_bytes: 65_535,
            max_extension_bytes: 4_096,
            local_ssrc: 1,
            remote_ssrc: None,
        }
    }
}

/// Errors raised while validating or driving an RTP session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionError {
    InvalidPayloadType(u8),
    InvalidClockRate,
    InvalidPacketLimit,
    PacketTooLarge { actual: usize, maximum: usize },
    UnexpectedPayloadType { expected: u8, actual: u8 },
    UnexpectedSsrc { expected: u32, actual: u32 },
    SourceAddressDenied { source: SocketAddr },
    Parse(ParseError),
    Serialize(SerializeError),
    Stats(StatsError),
}

impl Display for SessionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPayloadType(payload_type) => {
                write!(
                    formatter,
                    "RTP session payload type {payload_type} must fit in seven bits"
                )
            }
            Self::InvalidClockRate => {
                formatter.write_str("RTP session clock rate must be non-zero")
            }
            Self::InvalidPacketLimit => {
                formatter.write_str("RTP session packet limits must include the fixed header")
            }
            Self::PacketTooLarge { actual, maximum } => {
                write!(
                    formatter,
                    "RTP session packet is {actual} bytes, maximum is {maximum}"
                )
            }
            Self::UnexpectedPayloadType { expected, actual } => write!(
                formatter,
                "RTP session expected payload type {expected}, received {actual}"
            ),
            Self::UnexpectedSsrc { expected, actual } => {
                write!(
                    formatter,
                    "RTP session expected SSRC {expected}, received {actual}"
                )
            }
            Self::SourceAddressDenied { source } => {
                write!(formatter, "RTP source address {source} is not allowed")
            }
            Self::Parse(error) => Display::fmt(error, formatter),
            Self::Serialize(error) => Display::fmt(error, formatter),
            Self::Stats(error) => Display::fmt(error, formatter),
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

impl From<StatsError> for SessionError {
    fn from(error: StatsError) -> Self {
        Self::Stats(error)
    }
}

/// Aggregated RTP session counters and receive-side quality metrics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RtpSessionStats {
    pub packets_sent: u64,
    pub octets_sent: u64,
    pub received: RtpStats,
    pub last_received: Option<Duration>,
}

/// Stateful, bounded RTP send/receive session.
#[derive(Clone, Debug)]
pub struct RtpSession {
    config: RtpSessionConfig,
    source_policy: SourceIpPolicy,
    next_sequence: u16,
    next_timestamp: u32,
    remote_ssrc: Option<u32>,
    received: RtpStats,
    packets_sent: u64,
    octets_sent: u64,
    last_received: Option<Duration>,
}

impl RtpSession {
    /// Creates a session with deterministic initial sequence and timestamp values.
    pub fn new(
        config: RtpSessionConfig,
        initial_sequence: u16,
        initial_timestamp: u32,
    ) -> Result<Self, SessionError> {
        Self::new_with_source_policy(
            config,
            initial_sequence,
            initial_timestamp,
            SourceIpPolicy::default(),
        )
    }

    /// Creates a session with an explicit observed-source policy.
    ///
    /// # Errors
    ///
    /// Returns the same configuration errors as [`Self::new`].
    pub fn new_with_source_policy(
        config: RtpSessionConfig,
        initial_sequence: u16,
        initial_timestamp: u32,
        source_policy: SourceIpPolicy,
    ) -> Result<Self, SessionError> {
        if config.payload_type > 127 {
            return Err(SessionError::InvalidPayloadType(config.payload_type));
        }
        if config.clock_rate == 0 {
            return Err(SessionError::InvalidClockRate);
        }
        if !(12..=65_535).contains(&config.max_packet_bytes)
            || config.max_extension_bytes > config.max_packet_bytes.saturating_sub(12)
        {
            return Err(SessionError::InvalidPacketLimit);
        }
        Ok(Self {
            remote_ssrc: config.remote_ssrc,
            config,
            source_policy,
            next_sequence: initial_sequence,
            next_timestamp: initial_timestamp,
            received: RtpStats::default(),
            packets_sent: 0,
            octets_sent: 0,
            last_received: None,
        })
    }

    /// Replaces the observed-source policy while preserving session state.
    #[must_use]
    pub fn with_source_policy(mut self, source_policy: SourceIpPolicy) -> Self {
        self.source_policy = source_policy;
        self
    }

    /// Borrows the observed-source policy applied by source-aware receives.
    #[must_use]
    pub fn source_policy(&self) -> &SourceIpPolicy {
        &self.source_policy
    }

    /// Checks an observed source address before packet parsing or state changes.
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

    /// Serializes one RTP payload using the configured payload type and
    /// advances sequence/timestamp state.
    pub fn send(
        &mut self,
        payload: &[u8],
        timestamp_increment: u32,
        marker: bool,
    ) -> Result<Vec<u8>, SessionError> {
        self.send_with_payload_type(
            self.config.payload_type,
            payload,
            timestamp_increment,
            marker,
        )
    }

    /// Serializes one RTP payload with an explicitly selected payload type.
    ///
    /// This is useful for sessions that negotiate a telephone-event payload
    /// type alongside an audio payload type. Sequence and timestamp state are
    /// shared with regular audio packets, while the caller controls whether
    /// the timestamp advances (for example, DTMF retransmissions use zero).
    pub fn send_with_payload_type(
        &mut self,
        payload_type: u8,
        payload: &[u8],
        timestamp_increment: u32,
        marker: bool,
    ) -> Result<Vec<u8>, SessionError> {
        let wire = self.send_with_payload_type_at_timestamp(
            payload_type,
            payload,
            self.next_timestamp,
            marker,
        )?;
        self.next_timestamp = self.next_timestamp.wrapping_add(timestamp_increment);
        Ok(wire)
    }

    /// Serializes one RTP payload at an explicit timestamp without changing
    /// the session's next regular-media timestamp.
    ///
    /// Sequence and send-counter state still advance. This permits RFC 4733
    /// retransmissions to retain their event timestamp after the shared audio
    /// clock has moved forward.
    ///
    /// # Errors
    ///
    /// Returns an invalid payload-type, packet-bound, or serialization error.
    pub fn send_with_payload_type_at_timestamp(
        &mut self,
        payload_type: u8,
        payload: &[u8],
        timestamp: u32,
        marker: bool,
    ) -> Result<Vec<u8>, SessionError> {
        if payload_type > 127 {
            return Err(SessionError::InvalidPayloadType(payload_type));
        }
        let packet_bytes =
            12usize
                .checked_add(payload.len())
                .ok_or(SessionError::PacketTooLarge {
                    actual: usize::MAX,
                    maximum: self.config.max_packet_bytes,
                })?;
        if packet_bytes > self.config.max_packet_bytes {
            return Err(SessionError::PacketTooLarge {
                actual: packet_bytes,
                maximum: self.config.max_packet_bytes,
            });
        }
        let packet = RtpPacket {
            padding: false,
            marker,
            payload_type,
            sequence_number: self.next_sequence,
            timestamp,
            ssrc: self.config.local_ssrc,
            csrcs: Vec::new(),
            extension: None,
            payload: payload.to_vec(),
        };
        let wire = serialize(&packet)?;
        if wire.len() > self.config.max_packet_bytes {
            return Err(SessionError::PacketTooLarge {
                actual: wire.len(),
                maximum: self.config.max_packet_bytes,
            });
        }
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.packets_sent = self.packets_sent.saturating_add(1);
        self.octets_sent = self.octets_sent.saturating_add(payload.len() as u64);
        Ok(wire)
    }

    /// Replaces the timestamp used by the next regular RTP send.
    ///
    /// This is used when an explicitly timestamped RFC 4733 event finishes and
    /// regular audio must resume at the mapped end of that event.
    pub const fn synchronize_next_timestamp(&mut self, timestamp: u32) {
        self.next_timestamp = timestamp;
    }

    /// Parses and validates one received RTP packet, updating quality metrics.
    pub fn receive(&mut self, input: &[u8], arrival: Duration) -> Result<RtpPacket, SessionError> {
        let packet = match parse_with_config(
            input,
            ParseConfig {
                max_packet_bytes: self.config.max_packet_bytes,
                max_extension_bytes: self.config.max_extension_bytes,
            },
        ) {
            Ok(packet) => packet,
            Err(error) => {
                self.received.invalid_packets = self.received.invalid_packets.saturating_add(1);
                return Err(error.into());
            }
        };
        self.receive_packet(packet, arrival, self.config.payload_type)
    }

    /// Parses and validates one received RTP packet from an observed peer.
    ///
    /// The source policy is evaluated before parsing, so a denied peer cannot
    /// affect parse counters, SSRC state, sequence state, or receive metrics.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::SourceAddressDenied`] before parsing when the
    /// source is rejected, or forwards the usual RTP validation errors.
    pub fn receive_from(
        &mut self,
        input: &[u8],
        source: SocketAddr,
        arrival: Duration,
    ) -> Result<RtpPacket, SessionError> {
        self.authorize_source(source)?;
        self.receive(input, arrival)
    }

    /// Parses and validates one received RTP packet for an alternate payload
    /// type while preserving the session's shared SSRC and quality metrics.
    pub fn receive_with_payload_type(
        &mut self,
        input: &[u8],
        arrival: Duration,
        payload_type: u8,
    ) -> Result<RtpPacket, SessionError> {
        if payload_type > 127 {
            return Err(SessionError::InvalidPayloadType(payload_type));
        }
        let packet = match parse_with_config(
            input,
            ParseConfig {
                max_packet_bytes: self.config.max_packet_bytes,
                max_extension_bytes: self.config.max_extension_bytes,
            },
        ) {
            Ok(packet) => packet,
            Err(error) => {
                self.received.invalid_packets = self.received.invalid_packets.saturating_add(1);
                return Err(error.into());
            }
        };
        self.receive_packet(packet, arrival, payload_type)
    }

    /// Parses and validates an alternate payload type from an observed peer.
    ///
    /// The source policy is evaluated before parsing.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::SourceAddressDenied`] before parsing when the
    /// source is rejected, or forwards the usual RTP validation errors.
    pub fn receive_with_payload_type_from(
        &mut self,
        input: &[u8],
        source: SocketAddr,
        arrival: Duration,
        payload_type: u8,
    ) -> Result<RtpPacket, SessionError> {
        self.authorize_source(source)?;
        self.receive_with_payload_type(input, arrival, payload_type)
    }

    /// Validates a parsed RTP packet and updates quality metrics.
    pub fn receive_packet(
        &mut self,
        packet: RtpPacket,
        arrival: Duration,
        expected_payload_type: u8,
    ) -> Result<RtpPacket, SessionError> {
        if expected_payload_type > 127 {
            return Err(SessionError::InvalidPayloadType(expected_payload_type));
        }
        let extension_bytes = packet.extension.as_ref().map_or(0usize, |extension| {
            4usize.saturating_add(extension.data.len())
        });
        let packet_bytes = 12usize
            .checked_add(packet.csrcs.len().saturating_mul(4))
            .and_then(|size| size.checked_add(extension_bytes))
            .and_then(|size| size.checked_add(packet.payload.len()))
            .unwrap_or(usize::MAX);
        if packet_bytes > self.config.max_packet_bytes {
            self.received.invalid_packets = self.received.invalid_packets.saturating_add(1);
            return Err(SessionError::PacketTooLarge {
                actual: packet_bytes,
                maximum: self.config.max_packet_bytes,
            });
        }
        if packet.payload_type != expected_payload_type {
            self.received.invalid_packets = self.received.invalid_packets.saturating_add(1);
            return Err(SessionError::UnexpectedPayloadType {
                expected: expected_payload_type,
                actual: packet.payload_type,
            });
        }
        if let Some(expected) = self.config.remote_ssrc {
            if packet.ssrc != expected {
                self.received.invalid_packets = self.received.invalid_packets.saturating_add(1);
                return Err(SessionError::UnexpectedSsrc {
                    expected,
                    actual: packet.ssrc,
                });
            }
        }
        self.remote_ssrc = Some(packet.ssrc);
        self.received
            .observe(&packet, arrival, self.config.clock_rate)?;
        self.last_received = Some(arrival);
        Ok(packet)
    }

    /// Validates a parsed RTP packet from an observed peer.
    ///
    /// Callers that must inspect the payload type before dispatching (for
    /// example, a media session handling telephone-event) should authorize the
    /// source first, then parse, and finally call [`Self::receive_packet`].
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::SourceAddressDenied`] when the source is
    /// rejected, or forwards the usual RTP validation errors.
    pub fn receive_packet_from(
        &mut self,
        packet: RtpPacket,
        source: SocketAddr,
        arrival: Duration,
        expected_payload_type: u8,
    ) -> Result<RtpPacket, SessionError> {
        self.authorize_source(source)?;
        self.receive_packet(packet, arrival, expected_payload_type)
    }

    /// Returns a snapshot of sent and received metrics.
    pub fn stats(&self) -> RtpSessionStats {
        RtpSessionStats {
            packets_sent: self.packets_sent,
            octets_sent: self.octets_sent,
            received: self.received.clone(),
            last_received: self.last_received,
        }
    }

    /// Returns whether no packet has arrived within `timeout` at `now`.
    pub fn is_inactive(&self, now: Duration, timeout: Duration) -> bool {
        match self.last_received {
            Some(last) => now.saturating_sub(last) >= timeout,
            None => true,
        }
    }

    pub fn next_sequence(&self) -> u16 {
        self.next_sequence
    }

    pub fn next_timestamp(&self) -> u32 {
        self.next_timestamp
    }

    pub fn remote_ssrc(&self) -> Option<u32> {
        self.remote_ssrc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(sequence_number: u16, timestamp: u32, ssrc: u32) -> RtpPacket {
        RtpPacket {
            padding: false,
            marker: true,
            payload_type: 0,
            sequence_number,
            timestamp,
            ssrc,
            csrcs: Vec::new(),
            extension: None,
            payload: vec![1, 2, 3],
        }
    }

    #[test]
    fn parses_and_serializes_packet_with_extension() {
        let mut original = packet(7, 160, 42);
        original.csrcs = vec![1, 2];
        original.extension = Some(RtpExtension {
            profile: 0xBEDE,
            data: vec![0, 1, 2, 3],
        });
        let wire = serialize(&original).unwrap();
        assert_eq!(parse(&wire).unwrap(), original);
    }

    #[test]
    fn rejects_bad_version_padding_and_bounds() {
        assert!(matches!(
            parse(&[0; 12]),
            Err(ParseError::UnsupportedVersion(0))
        ));
        let mut invalid = serialize(&packet(1, 0, 1)).unwrap();
        invalid[0] |= 0x20;
        invalid.push(0);
        assert!(matches!(parse(&invalid), Err(ParseError::InvalidPadding)));
        assert!(matches!(parse(&[0; 11]), Err(ParseError::TooShort)));
        let mut invalid_payload_type = packet(1, 0, 1);
        invalid_payload_type.payload_type = 128;
        assert!(matches!(
            serialize(&invalid_payload_type),
            Err(SerializeError::InvalidPayloadType)
        ));
    }

    #[test]
    fn tracks_loss_jitter_ssrc_changes_and_rollover() {
        let mut stats = RtpStats::default();
        stats
            .observe(&packet(u16::MAX, 0, 1), Duration::from_millis(0), 8_000)
            .unwrap();
        stats
            .observe(&packet(0, 160, 1), Duration::from_millis(20), 8_000)
            .unwrap();
        stats
            .observe(&packet(2, 240, 1), Duration::from_millis(60), 8_000)
            .unwrap();
        stats
            .observe(&packet(10, 400, 2), Duration::from_millis(90), 8_000)
            .unwrap();
        stats
            .observe(&packet(11, 480, 2), Duration::from_millis(110), 8_000)
            .unwrap();
        assert_eq!(stats.packets_lost, 1);
        assert_eq!(stats.ssrc_changes, 1);
        assert!(stats.jitter > 0);
    }

    #[test]
    fn session_advances_send_state_and_tracks_receive_metrics() {
        let mut session = RtpSession::new(
            RtpSessionConfig {
                local_ssrc: 7,
                ..RtpSessionConfig::default()
            },
            u16::MAX,
            u32::MAX - 80,
        )
        .unwrap();
        let first = session.send(&[1, 2, 3], 160, true).unwrap();
        assert_eq!(session.next_sequence(), 0);
        assert_eq!(session.next_timestamp(), 79);
        let packet = session.receive(&first, Duration::from_millis(10)).unwrap();
        assert_eq!(packet.sequence_number, u16::MAX);
        assert_eq!(session.remote_ssrc(), Some(7));
        assert_eq!(session.stats().packets_sent, 1);
        assert_eq!(session.stats().octets_sent, 3);
        assert_eq!(session.stats().received.packets_received, 1);
        assert!(!session.is_inactive(Duration::from_millis(10), Duration::from_millis(1)));
        assert!(session.is_inactive(Duration::from_millis(20), Duration::from_millis(10)));
    }

    #[test]
    fn session_rejects_unexpected_payload_source_and_limits() {
        assert!(matches!(
            RtpSession::new(
                RtpSessionConfig {
                    payload_type: 128,
                    ..RtpSessionConfig::default()
                },
                0,
                0
            ),
            Err(SessionError::InvalidPayloadType(128))
        ));
        let mut session = RtpSession::new(
            RtpSessionConfig {
                remote_ssrc: Some(42),
                ..RtpSessionConfig::default()
            },
            0,
            0,
        )
        .unwrap();
        let mut packet = packet(1, 0, 7);
        packet.payload_type = 8;
        let wire = serialize(&packet).unwrap();
        assert!(matches!(
            session.receive(&wire, Duration::ZERO),
            Err(SessionError::UnexpectedPayloadType {
                expected: 0,
                actual: 8
            })
        ));
        assert_eq!(session.stats().received.invalid_packets, 1);
        packet.payload_type = 0;
        packet.ssrc = 7;
        let wire = serialize(&packet).unwrap();
        assert!(matches!(
            session.receive(&wire, Duration::ZERO),
            Err(SessionError::UnexpectedSsrc {
                expected: 42,
                actual: 7
            })
        ));
        assert_eq!(session.stats().received.invalid_packets, 2);
    }

    #[test]
    fn source_policy_rejects_before_parse_and_state_mutation() {
        let mut policy = SourceIpPolicy::default();
        policy.add_allow("198.51.100.0/24").unwrap();
        policy.add_deny("198.51.100.128/25").unwrap();
        let mut session =
            RtpSession::new_with_source_policy(RtpSessionConfig::default(), 9, 900, policy)
                .unwrap();
        let baseline = session.stats();
        let denied = "198.51.100.200:4000".parse().unwrap();
        assert_eq!(
            session.receive_from(&[], denied, Duration::ZERO),
            Err(SessionError::SourceAddressDenied { source: denied })
        );
        assert_eq!(session.stats(), baseline);
        assert_eq!(session.remote_ssrc(), None);
        assert_eq!(session.next_sequence(), 9);
        assert_eq!(session.next_timestamp(), 900);

        let allowed = "198.51.100.10:4000".parse().unwrap();
        let wire = serialize(&packet(1, 80, 42)).unwrap();
        session
            .receive_from(&wire, allowed, Duration::from_millis(10))
            .unwrap();
        assert_eq!(session.stats().received.packets_received, 1);
        assert_eq!(session.remote_ssrc(), Some(42));
    }

    #[test]
    fn source_policy_keeps_ipv4_and_ipv6_families_separate() {
        let mut policy = SourceIpPolicy::default();
        policy.add_allow("2001:db8::/32").unwrap();
        let mut session =
            RtpSession::new_with_source_policy(RtpSessionConfig::default(), 0, 0, policy).unwrap();
        let wire = serialize(&packet(1, 0, 7)).unwrap();
        let ipv6 = "[2001:db8::10]:4000".parse().unwrap();
        session.receive_from(&wire, ipv6, Duration::ZERO).unwrap();
        let before_ipv4 = session.stats();
        let ipv4 = "192.0.2.10:4000".parse().unwrap();
        assert_eq!(
            session.receive_from(&wire, ipv4, Duration::from_millis(1)),
            Err(SessionError::SourceAddressDenied { source: ipv4 })
        );
        assert_eq!(session.stats(), before_ipv4);
    }

    #[test]
    fn session_can_share_sequence_state_with_an_alternate_payload_type() {
        let mut session = RtpSession::new(
            RtpSessionConfig {
                payload_type: 0,
                ..RtpSessionConfig::default()
            },
            12,
            4_000,
        )
        .unwrap();
        let wire = session
            .send_with_payload_type(101, &[5, 6, 7, 8], 0, true)
            .unwrap();
        let packet = parse(&wire).unwrap();
        assert_eq!(packet.payload_type, 101);
        assert_eq!(packet.sequence_number, 12);
        assert_eq!(session.next_sequence(), 13);
        let mut receiver = RtpSession::new(
            RtpSessionConfig {
                payload_type: 0,
                ..RtpSessionConfig::default()
            },
            1,
            1,
        )
        .unwrap();
        assert_eq!(
            receiver
                .receive_packet(packet, Duration::from_millis(1), 101)
                .unwrap()
                .payload_type,
            101
        );
        assert_eq!(receiver.stats().received.packets_received, 1);
    }

    #[test]
    fn explicit_timestamp_preserves_regular_clock_while_advancing_sequence() {
        let mut session = RtpSession::new(RtpSessionConfig::default(), 12, 4_000).unwrap();
        let event = session
            .send_with_payload_type_at_timestamp(101, &[5, 0x8a, 0, 80], 9_000, true)
            .unwrap();
        let packet = parse(&event).unwrap();
        assert_eq!(packet.sequence_number, 12);
        assert_eq!(packet.timestamp, 9_000);
        assert_eq!(session.next_sequence(), 13);
        assert_eq!(session.next_timestamp(), 4_000);

        session.synchronize_next_timestamp(9_160);
        let audio = parse(&session.send(&[0xff; 160], 160, false).unwrap()).unwrap();
        assert_eq!(audio.sequence_number, 13);
        assert_eq!(audio.timestamp, 9_160);
        assert_eq!(session.next_timestamp(), 9_320);
    }
}
