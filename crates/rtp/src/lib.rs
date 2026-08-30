//! Safe RTP packet parsing, serialization, and session statistics.

use std::{
    error::Error,
    fmt::{Display, Formatter},
    time::Duration,
};

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
}
