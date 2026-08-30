//! Bounded media queues and G.711 conversion primitives.

use std::{
    collections::VecDeque,
    error::Error,
    fmt::{Display, Formatter},
};

use rtp::PayloadCodec;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DropPolicy {
    DropOldest,
    DropNewest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PushOutcome {
    Accepted,
    DroppedOldest,
    DroppedNewest,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QueueError {
    ZeroCapacity,
}

impl Display for QueueError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("media queue capacity must be greater than zero")
    }
}

impl Error for QueueError {}

#[derive(Clone, Debug)]
pub struct BoundedMediaQueue<T> {
    items: VecDeque<T>,
    capacity: usize,
    policy: DropPolicy,
    pushed: u64,
    dropped_oldest: u64,
    dropped_newest: u64,
}

impl<T> BoundedMediaQueue<T> {
    pub fn new(capacity: usize, policy: DropPolicy) -> Result<Self, QueueError> {
        if capacity == 0 {
            return Err(QueueError::ZeroCapacity);
        }
        Ok(Self {
            items: VecDeque::with_capacity(capacity),
            capacity,
            policy,
            pushed: 0,
            dropped_oldest: 0,
            dropped_newest: 0,
        })
    }

    pub fn push(&mut self, item: T) -> PushOutcome {
        self.pushed = self.pushed.saturating_add(1);
        if self.items.len() < self.capacity {
            self.items.push_back(item);
            return PushOutcome::Accepted;
        }
        match self.policy {
            DropPolicy::DropOldest => {
                let _ = self.items.pop_front();
                self.items.push_back(item);
                self.dropped_oldest = self.dropped_oldest.saturating_add(1);
                PushOutcome::DroppedOldest
            }
            DropPolicy::DropNewest => {
                self.dropped_newest = self.dropped_newest.saturating_add(1);
                PushOutcome::DroppedNewest
            }
        }
    }

    pub fn pop(&mut self) -> Option<T> {
        self.items.pop_front()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn stats(&self) -> QueueStats {
        QueueStats {
            depth: self.items.len(),
            capacity: self.capacity,
            pushed: self.pushed,
            dropped_oldest: self.dropped_oldest,
            dropped_newest: self.dropped_newest,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QueueStats {
    pub depth: usize,
    pub capacity: usize,
    pub pushed: u64,
    pub dropped_oldest: u64,
    pub dropped_newest: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioCodec {
    Pcmu,
    Pcma,
}

impl AudioCodec {
    pub fn payload_codec(self) -> PayloadCodec {
        match self {
            Self::Pcmu => PayloadCodec::Pcmu,
            Self::Pcma => PayloadCodec::Pcma,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioFrame {
    pub timestamp: u32,
    pub codec: AudioCodec,
    pub sample_rate: u32,
    pub samples: Vec<i16>,
}

pub fn decode(codec: AudioCodec, encoded: &[u8]) -> Vec<i16> {
    encoded
        .iter()
        .map(|sample| match codec {
            AudioCodec::Pcmu => ulaw_to_linear(*sample),
            AudioCodec::Pcma => alaw_to_linear(*sample),
        })
        .collect()
}

pub fn encode(codec: AudioCodec, samples: &[i16]) -> Vec<u8> {
    samples
        .iter()
        .map(|sample| match codec {
            AudioCodec::Pcmu => linear_to_ulaw(*sample),
            AudioCodec::Pcma => linear_to_alaw(*sample),
        })
        .collect()
}

pub fn ulaw_to_linear(encoded: u8) -> i16 {
    let value = i32::from(!encoded);
    let mut sample = ((value & 0x0f) << 3) + 0x84;
    sample <<= (value & 0x70) >> 4;
    sample = if value & 0x80 != 0 {
        0x84 - sample
    } else {
        sample - 0x84
    };
    sample.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

pub fn linear_to_ulaw(sample: i16) -> u8 {
    const BIAS: i32 = 0x84;
    const CLIP: i32 = 32635;
    let mut value = i32::from(sample);
    let sign = if value < 0 {
        value = -value;
        0x80
    } else {
        0
    };
    value = value.min(CLIP) + BIAS;
    let mut exponent = 7;
    // Segment boundaries are powers of two starting at 2^7 (the exponent
    // field is effectively the high bit position after the u-law bias).
    while exponent > 0 && value < (0x80 << exponent) {
        exponent -= 1;
    }
    let mantissa = (value >> (exponent + 3)) & 0x0f;
    !(sign | (exponent << 4) | mantissa) as u8
}

pub fn alaw_to_linear(encoded: u8) -> i16 {
    let value = encoded ^ 0x55;
    let mut sample = i32::from(value & 0x0f) << 4;
    let segment = i32::from(value & 0x70) >> 4;
    if segment == 0 {
        sample += 8;
    } else if segment == 1 {
        sample += 0x108;
    } else {
        sample += 0x108;
        sample <<= segment - 1;
    }
    if value & 0x80 == 0 {
        sample = -sample;
    }
    sample.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

pub fn linear_to_alaw(sample: i16) -> u8 {
    let sign = if sample >= 0 { 0x80 } else { 0 };
    let magnitude = if sample < 0 {
        -i32::from(sample) - 1
    } else {
        i32::from(sample)
    }
    .max(0);
    let (segment, mantissa) = if magnitude >= 256 {
        let segment = (31 - magnitude.leading_zeros() as i32).saturating_sub(7);
        (segment, (magnitude >> (segment + 3)) & 0x0f)
    } else {
        (0, magnitude >> 4)
    };
    (sign | (segment << 4) | mantissa) as u8 ^ 0x55
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_enforces_capacity_and_reports_policy() {
        let mut queue = BoundedMediaQueue::new(2, DropPolicy::DropOldest).unwrap();
        assert_eq!(queue.push(1), PushOutcome::Accepted);
        assert_eq!(queue.push(2), PushOutcome::Accepted);
        assert_eq!(queue.push(3), PushOutcome::DroppedOldest);
        assert_eq!(queue.pop(), Some(2));
        assert_eq!(queue.stats().dropped_oldest, 1);
        assert!(BoundedMediaQueue::<u8>::new(0, DropPolicy::DropNewest).is_err());
    }

    #[test]
    fn g711_silence_and_signal_round_trip_with_bounded_error() {
        assert_eq!(linear_to_ulaw(0), 0xff);
        assert_eq!(linear_to_alaw(0), 0xd5);
        assert_eq!(linear_to_ulaw(1_000), 0xce);
        assert_eq!(linear_to_ulaw(-1_000), 0x4e);
        assert_eq!(linear_to_ulaw(30_000), 0x82);
        assert_eq!(linear_to_ulaw(-30_000), 0x02);
        assert_eq!(linear_to_alaw(1_000), 0xfa);
        assert_eq!(linear_to_alaw(-1_000), 0x7a);
        for sample in [-30_000, -1_000, 0, 1_000, 30_000] {
            let ulaw = ulaw_to_linear(linear_to_ulaw(sample));
            let alaw = alaw_to_linear(linear_to_alaw(sample));
            assert!((i32::from(ulaw) - i32::from(sample)).abs() < 2_500);
            assert!((i32::from(alaw) - i32::from(sample)).abs() < 2_500);
        }
    }
}
