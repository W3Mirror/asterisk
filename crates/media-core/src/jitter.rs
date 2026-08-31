//! Bounded, fixed-delay RTP audio playout ordering.

use std::{collections::BTreeMap, time::Duration};

use rtp::RtpPacket;

const MAX_JITTER_PACKETS: usize = 4_096;
const MAX_PLAYOUT_DELAY: Duration = Duration::from_secs(10);

/// Resource and timing bounds for one receive-side audio jitter buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JitterBufferConfig {
    /// Maximum number of validated audio packets retained before playout.
    /// Values above 4,096 are rejected.
    pub max_packets: usize,
    /// Fixed delay added to the first packet's arrival time. Zero and values
    /// above ten seconds are rejected.
    pub playout_delay: Duration,
}

impl Default for JitterBufferConfig {
    fn default() -> Self {
        Self {
            max_packets: 16,
            playout_delay: Duration::from_millis(60),
        }
    }
}

impl JitterBufferConfig {
    pub(crate) fn is_valid(self) -> bool {
        (1..=MAX_JITTER_PACKETS).contains(&self.max_packets)
            && !self.playout_delay.is_zero()
            && self.playout_delay <= MAX_PLAYOUT_DELAY
    }
}

/// Result of offering one validated audio packet to a jitter buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JitterPushOutcome {
    /// The packet was retained for time-based playout.
    Accepted,
    /// An identical RTP timestamp/sequence position was already retained.
    DroppedDuplicate,
    /// The packet belongs before audio that has already played.
    DroppedLate,
    /// The packet was the farthest-future packet at the configured bound.
    DroppedOverflow,
    /// A farther-future retained packet was replaced to protect imminent audio.
    ReplacedFuture,
    /// A new SSRC reset the old source's buffered timeline.
    SourceReset,
}

/// Constant-size jitter-buffer counters and current depth.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct JitterBufferStats {
    /// Packets currently retained for playout.
    pub depth: usize,
    /// Configured packet capacity.
    pub capacity: usize,
    /// Packets accepted into the buffer, including source-reset packets.
    pub accepted: u64,
    /// Packets emitted in RTP timestamp order.
    pub played: u64,
    /// Duplicate packets discarded before playout.
    pub dropped_duplicate: u64,
    /// Packets discarded because their playout position had passed.
    pub dropped_late: u64,
    /// Incoming or retained far-future packets discarded at capacity.
    pub dropped_overflow: u64,
    /// Buffered packets discarded when the remote SSRC changed.
    pub dropped_on_source_reset: u64,
    /// Number of observed remote SSRC changes.
    pub source_resets: u64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct PlayoutKey {
    timestamp: i64,
    sequence: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Timeline {
    timestamp: i64,
    deadline: Duration,
}

/// Fixed-delay audio packet buffer driven entirely by caller-supplied time.
#[derive(Clone, Debug)]
pub(crate) struct AudioJitterBuffer {
    config: JitterBufferConfig,
    packets: BTreeMap<PlayoutKey, RtpPacket>,
    source_ssrc: Option<u32>,
    highest_timestamp: Option<i64>,
    highest_sequence: Option<i64>,
    last_played: Option<PlayoutKey>,
    timeline: Option<Timeline>,
    stats: JitterBufferStats,
}

impl AudioJitterBuffer {
    pub(crate) fn new(config: JitterBufferConfig) -> Self {
        Self {
            packets: BTreeMap::new(),
            source_ssrc: None,
            highest_timestamp: None,
            highest_sequence: None,
            last_played: None,
            timeline: None,
            stats: JitterBufferStats {
                capacity: config.max_packets,
                ..JitterBufferStats::default()
            },
            config,
        }
    }

    pub(crate) fn push(&mut self, packet: RtpPacket, arrival: Duration) -> JitterPushOutcome {
        let source_reset = self
            .source_ssrc
            .is_some_and(|source_ssrc| source_ssrc != packet.ssrc);
        if source_reset {
            self.stats.dropped_on_source_reset = self
                .stats
                .dropped_on_source_reset
                .saturating_add(self.packets.len() as u64);
            self.stats.source_resets = self.stats.source_resets.saturating_add(1);
            self.packets.clear();
            self.highest_timestamp = None;
            self.highest_sequence = None;
            self.last_played = None;
            self.timeline = None;
        }
        self.source_ssrc = Some(packet.ssrc);

        let timestamp = extend_u32(self.highest_timestamp, packet.timestamp);
        let sequence = extend_u16(self.highest_sequence, packet.sequence_number);
        self.highest_timestamp = Some(
            self.highest_timestamp
                .map_or(timestamp, |value| value.max(timestamp)),
        );
        self.highest_sequence = Some(
            self.highest_sequence
                .map_or(sequence, |value| value.max(sequence)),
        );
        let key = PlayoutKey {
            timestamp,
            sequence,
        };

        if self.last_played.is_some_and(|played| key <= played) {
            self.stats.dropped_late = self.stats.dropped_late.saturating_add(1);
            return JitterPushOutcome::DroppedLate;
        }
        if self.packets.contains_key(&key) {
            self.stats.dropped_duplicate = self.stats.dropped_duplicate.saturating_add(1);
            return JitterPushOutcome::DroppedDuplicate;
        }

        if packet.marker && self.packets.is_empty() && self.last_played.is_some() {
            // RTP's marker bit starts a new talkspurt for G.711. Re-anchor an
            // empty buffer so a sender timestamp discontinuity during silence
            // cannot schedule the new talkspurt arbitrarily far away.
            self.timeline = None;
        }
        self.timeline.get_or_insert(Timeline {
            timestamp,
            deadline: arrival.saturating_add(self.config.playout_delay),
        });

        let mut outcome = if source_reset {
            JitterPushOutcome::SourceReset
        } else {
            JitterPushOutcome::Accepted
        };
        if self.packets.len() == self.config.max_packets {
            let Some((&farthest, _)) = self.packets.last_key_value() else {
                self.stats.dropped_overflow = self.stats.dropped_overflow.saturating_add(1);
                self.refresh_depth();
                return JitterPushOutcome::DroppedOverflow;
            };
            self.stats.dropped_overflow = self.stats.dropped_overflow.saturating_add(1);
            if key >= farthest {
                self.refresh_depth();
                return JitterPushOutcome::DroppedOverflow;
            }
            let _ = self.packets.remove(&farthest);
            outcome = JitterPushOutcome::ReplacedFuture;
        }
        self.packets.insert(key, packet);
        self.stats.accepted = self.stats.accepted.saturating_add(1);
        self.refresh_depth();
        outcome
    }

    pub(crate) fn pop_due(&mut self, now: Duration, clock_rate: u32) -> Option<RtpPacket> {
        let (&key, _) = self.packets.first_key_value()?;
        if now < self.deadline(key.timestamp, clock_rate)? {
            return None;
        }
        let packet = self.packets.remove(&key)?;
        self.last_played = Some(key);
        self.stats.played = self.stats.played.saturating_add(1);
        self.refresh_depth();
        Some(packet)
    }

    pub(crate) fn next_deadline(&self, clock_rate: u32) -> Option<Duration> {
        let (&key, _) = self.packets.first_key_value()?;
        self.deadline(key.timestamp, clock_rate)
    }

    pub(crate) fn stats(&self) -> JitterBufferStats {
        self.stats
    }

    pub(crate) fn clear(&mut self) -> usize {
        let released = self.packets.len();
        self.packets.clear();
        self.source_ssrc = None;
        self.highest_timestamp = None;
        self.highest_sequence = None;
        self.last_played = None;
        self.timeline = None;
        self.refresh_depth();
        released
    }

    fn deadline(&self, timestamp: i64, clock_rate: u32) -> Option<Duration> {
        let timeline = self.timeline?;
        let signed_ticks = timestamp.saturating_sub(timeline.timestamp);
        let ticks = signed_ticks.unsigned_abs();
        let clock_rate = u64::from(clock_rate);
        let seconds = ticks / clock_rate;
        let fractional_ticks = ticks % clock_rate;
        let nanos = fractional_ticks.saturating_mul(1_000_000_000) / clock_rate;
        let magnitude = Duration::new(seconds, u32::try_from(nanos).unwrap_or(999_999_999));
        if signed_ticks >= 0 {
            Some(timeline.deadline.saturating_add(magnitude))
        } else {
            Some(timeline.deadline.saturating_sub(magnitude))
        }
    }

    fn refresh_depth(&mut self) {
        self.stats.depth = self.packets.len();
    }
}

fn extend_u16(highest: Option<i64>, value: u16) -> i64 {
    extend_wrapping(highest, u64::from(value), 16)
}

fn extend_u32(highest: Option<i64>, value: u32) -> i64 {
    extend_wrapping(highest, u64::from(value), 32)
}

fn extend_wrapping(highest: Option<i64>, value: u64, bits: u32) -> i64 {
    let Some(highest) = highest else {
        return i64::try_from(value).unwrap_or(i64::MAX);
    };
    let modulus = 1_i64 << bits;
    let mask = modulus - 1;
    let half = modulus / 2;
    let mut extended = (highest & !mask) | i64::try_from(value).unwrap_or(mask);
    let difference = extended.saturating_sub(highest);
    if difference > half {
        extended = extended.saturating_sub(modulus);
    } else if difference < -half {
        extended = extended.saturating_add(modulus);
    }
    extended
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(sequence: u16, timestamp: u32, ssrc: u32) -> RtpPacket {
        RtpPacket {
            padding: false,
            marker: false,
            payload_type: 0,
            sequence_number: sequence,
            timestamp,
            ssrc,
            csrcs: Vec::new(),
            extension: None,
            payload: vec![sequence.to_le_bytes()[0]],
        }
    }

    #[test]
    fn reorders_packets_until_their_fixed_playout_deadlines() {
        let mut jitter = AudioJitterBuffer::new(JitterBufferConfig {
            max_packets: 4,
            playout_delay: Duration::from_millis(60),
        });
        assert_eq!(
            jitter.push(packet(11, 1_160, 7), Duration::from_millis(20)),
            JitterPushOutcome::Accepted
        );
        assert_eq!(
            jitter.push(packet(10, 1_000, 7), Duration::from_millis(30)),
            JitterPushOutcome::Accepted
        );
        assert_eq!(jitter.next_deadline(8_000), Some(Duration::from_millis(60)));
        assert!(jitter.pop_due(Duration::from_millis(59), 8_000).is_none());
        assert_eq!(
            jitter
                .pop_due(Duration::from_millis(60), 8_000)
                .unwrap()
                .sequence_number,
            10
        );
        assert_eq!(
            jitter
                .pop_due(Duration::from_millis(80), 8_000)
                .unwrap()
                .sequence_number,
            11
        );
    }

    #[test]
    fn preserves_imminent_audio_when_the_packet_bound_is_full() {
        let mut jitter = AudioJitterBuffer::new(JitterBufferConfig {
            max_packets: 2,
            ..JitterBufferConfig::default()
        });
        assert_eq!(
            jitter.push(packet(2, 320, 7), Duration::ZERO),
            JitterPushOutcome::Accepted
        );
        assert_eq!(
            jitter.push(packet(3, 480, 7), Duration::ZERO),
            JitterPushOutcome::Accepted
        );
        assert_eq!(
            jitter.push(packet(1, 160, 7), Duration::ZERO),
            JitterPushOutcome::ReplacedFuture
        );
        assert_eq!(
            jitter.push(packet(4, 640, 7), Duration::ZERO),
            JitterPushOutcome::DroppedOverflow
        );
        assert_eq!(jitter.stats().depth, 2);
        assert_eq!(jitter.stats().dropped_overflow, 2);
    }

    #[test]
    fn rejects_duplicates_and_late_packets_and_resets_on_source_change() {
        let mut jitter = AudioJitterBuffer::new(JitterBufferConfig::default());
        let first = packet(u16::MAX, u32::MAX - 79, 7);
        assert_eq!(
            jitter.push(first.clone(), Duration::ZERO),
            JitterPushOutcome::Accepted
        );
        assert_eq!(
            jitter.push(first, Duration::ZERO),
            JitterPushOutcome::DroppedDuplicate
        );
        assert_eq!(
            jitter.push(packet(0, 80, 7), Duration::ZERO),
            JitterPushOutcome::Accepted
        );
        assert_eq!(
            jitter
                .pop_due(Duration::from_millis(60), 8_000)
                .unwrap()
                .sequence_number,
            u16::MAX
        );
        assert_eq!(
            jitter.push(packet(u16::MAX - 1, u32::MAX - 239, 7), Duration::ZERO),
            JitterPushOutcome::DroppedLate
        );
        assert_eq!(
            jitter.push(packet(1, 160, 8), Duration::from_millis(70)),
            JitterPushOutcome::SourceReset
        );
        assert_eq!(jitter.stats().source_resets, 1);
        assert_eq!(jitter.stats().dropped_on_source_reset, 1);
    }

    #[test]
    fn marker_reanchors_an_empty_buffer_after_a_timestamp_discontinuity() {
        let mut jitter = AudioJitterBuffer::new(JitterBufferConfig::default());
        jitter.push(packet(1, 160, 7), Duration::ZERO);
        assert!(jitter.pop_due(Duration::from_millis(60), 8_000).is_some());
        let mut restarted = packet(2, 80_000, 7);
        restarted.marker = true;
        jitter.push(restarted, Duration::from_secs(1));
        assert_eq!(
            jitter.next_deadline(8_000),
            Some(Duration::from_millis(1_060))
        );
    }
}
