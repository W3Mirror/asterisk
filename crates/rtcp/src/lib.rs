//! Safe parsing of the RTCP sender/receiver reports needed for observability.

use std::{
    error::Error,
    fmt::{Display, Formatter},
};

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
    if input.len() > 65_535 {
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
}
