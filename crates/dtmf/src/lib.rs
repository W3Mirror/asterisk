//! RFC 4733 telephone-event parsing, generation, and duplicate suppression.

use std::{
    error::Error,
    fmt::{Display, Formatter},
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DtmfDigit {
    Zero,
    One,
    Two,
    Three,
    Four,
    Five,
    Six,
    Seven,
    Eight,
    Nine,
    Star,
    Pound,
    A,
    B,
    C,
    D,
    Flash,
}

impl DtmfDigit {
    pub fn event_code(self) -> u8 {
        match self {
            Self::Zero => 0,
            Self::One => 1,
            Self::Two => 2,
            Self::Three => 3,
            Self::Four => 4,
            Self::Five => 5,
            Self::Six => 6,
            Self::Seven => 7,
            Self::Eight => 8,
            Self::Nine => 9,
            Self::Star => 10,
            Self::Pound => 11,
            Self::A => 12,
            Self::B => 13,
            Self::C => 14,
            Self::D => 15,
            Self::Flash => 16,
        }
    }

    fn from_event_code(code: u8) -> Option<Self> {
        Some(match code {
            0 => Self::Zero,
            1 => Self::One,
            2 => Self::Two,
            3 => Self::Three,
            4 => Self::Four,
            5 => Self::Five,
            6 => Self::Six,
            7 => Self::Seven,
            8 => Self::Eight,
            9 => Self::Nine,
            10 => Self::Star,
            11 => Self::Pound,
            12 => Self::A,
            13 => Self::B,
            14 => Self::C,
            15 => Self::D,
            16 => Self::Flash,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DtmfEvent {
    pub digit: DtmfDigit,
    pub end: bool,
    pub reserved: bool,
    pub volume: u8,
    pub duration: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParseError {
    TooShort,
    InvalidLength,
    UnsupportedEvent(u8),
    InvalidVolume(u8),
    InvalidDuration,
}

impl Display for ParseError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooShort => formatter.write_str("telephone-event payload must be four bytes"),
            Self::InvalidLength => {
                formatter.write_str("telephone-event payload must be exactly four bytes")
            }
            Self::UnsupportedEvent(event) => {
                write!(formatter, "unsupported telephone-event {event}")
            }
            Self::InvalidVolume(volume) => {
                write!(formatter, "telephone-event volume {volume} exceeds 63")
            }
            Self::InvalidDuration => {
                formatter.write_str("telephone-event duration must be non-zero")
            }
        }
    }
}

impl Error for ParseError {}

pub fn parse(payload: &[u8]) -> Result<DtmfEvent, ParseError> {
    if payload.len() < 4 {
        return Err(ParseError::TooShort);
    }
    if payload.len() > 4 {
        return Err(ParseError::InvalidLength);
    }
    let digit =
        DtmfDigit::from_event_code(payload[0]).ok_or(ParseError::UnsupportedEvent(payload[0]))?;
    let flags = payload[1];
    let volume = flags & 0x3f;
    if volume > 63 {
        return Err(ParseError::InvalidVolume(volume));
    }
    let duration = u16::from_be_bytes([payload[2], payload[3]]);
    if duration == 0 {
        return Err(ParseError::InvalidDuration);
    }
    Ok(DtmfEvent {
        digit,
        end: flags & 0x80 != 0,
        reserved: flags & 0x40 != 0,
        volume,
        duration,
    })
}

pub fn encode(event: DtmfEvent) -> Result<[u8; 4], EncodeError> {
    if event.volume > 63 {
        return Err(EncodeError::InvalidVolume);
    }
    if event.duration == 0 {
        return Err(EncodeError::InvalidDuration);
    }
    let mut flags = event.volume;
    if event.end {
        flags |= 0x80;
    }
    if event.reserved {
        flags |= 0x40;
    }
    Ok([
        event.digit.event_code(),
        flags,
        (event.duration >> 8) as u8,
        event.duration as u8,
    ])
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EncodeError {
    InvalidVolume,
    InvalidDuration,
}

impl Display for EncodeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidVolume => "telephone-event volume exceeds 63",
            Self::InvalidDuration => "telephone-event duration must be non-zero",
        })
    }
}

impl Error for EncodeError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Notification {
    Started(DtmfDigit),
    Ended { digit: DtmfDigit, duration: u16 },
}

#[derive(Clone, Debug, Default)]
pub struct Deduplicator {
    active: Option<(DtmfDigit, u16)>,
    last_ended: Option<(DtmfDigit, u16)>,
}

impl Deduplicator {
    pub fn observe(&mut self, event: DtmfEvent) -> Option<Notification> {
        if event.end {
            if self
                .last_ended
                .is_some_and(|(digit, _)| self.active.is_none() && digit == event.digit)
            {
                return None;
            }
            self.active = None;
            self.last_ended = Some((event.digit, event.duration));
            return Some(Notification::Ended {
                digit: event.digit,
                duration: event.duration,
            });
        }

        self.last_ended = None;
        match self.active {
            Some((active_digit, _)) if active_digit == event.digit => {
                self.active = Some((active_digit, event.duration));
                None
            }
            Some(_) => {
                self.active = Some((event.digit, event.duration));
                Some(Notification::Started(event.digit))
            }
            None => {
                self.active = Some((event.digit, event.duration));
                Some(Notification::Started(event.digit))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(end: bool, duration: u16) -> DtmfEvent {
        DtmfEvent {
            digit: DtmfDigit::Five,
            end,
            reserved: false,
            volume: 10,
            duration,
        }
    }

    #[test]
    fn telephone_event_round_trips() {
        let original = event(true, 160);
        assert_eq!(parse(&encode(original).unwrap()).unwrap(), original);
    }

    #[test]
    fn duplicate_packets_emit_one_start_and_one_end() {
        let mut deduplicator = Deduplicator::default();
        assert_eq!(
            deduplicator.observe(event(false, 80)),
            Some(Notification::Started(DtmfDigit::Five))
        );
        assert_eq!(deduplicator.observe(event(false, 120)), None);
        assert_eq!(
            deduplicator.observe(event(true, 160)),
            Some(Notification::Ended {
                digit: DtmfDigit::Five,
                duration: 160
            })
        );
        assert_eq!(deduplicator.observe(event(true, 160)), None);
    }

    #[test]
    fn end_only_packets_emit_once_and_duplicates_are_suppressed() {
        let mut deduplicator = Deduplicator::default();
        assert_eq!(
            deduplicator.observe(event(true, 160)),
            Some(Notification::Ended {
                digit: DtmfDigit::Five,
                duration: 160
            })
        );
        assert_eq!(deduplicator.observe(event(true, 200)), None);
    }

    #[test]
    fn malformed_payloads_never_panic() {
        for length in 0..128 {
            let payload = vec![length as u8; length];
            let _ = parse(&payload);
        }
        assert!(matches!(
            parse(&[5, 0, 0, 1, 0]),
            Err(ParseError::InvalidLength)
        ));
    }
}
