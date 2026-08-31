//! Deterministic bounded media load and reclamation smoke testing.

use std::{
    error::Error,
    fmt::{Display, Formatter},
    fs,
    time::{Duration, Instant},
};

use media_core::{
    AudioCodec, AudioFrame, DropPolicy, JitterBufferConfig, JitterPushOutcome, MediaBridgeConfig,
    MediaSession, MediaSessionConfig, MediaSessionError, PushOutcome, ReceivedMedia,
};
use rtp::{RtpPacket, RtpSessionConfig, SerializeError, serialize};

const AUDIO_SAMPLES: usize = 160;
const PACKET_INTERVAL: Duration = Duration::from_millis(20);
const PLAYOUT_DELAY: Duration = Duration::from_millis(60);
const MAX_STREAMS: usize = 1_000_000;
const MAX_PACKETS_PER_STREAM: usize = 1_000_000;

/// Bounds for one deterministic bidirectional media smoke run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MediaSmokeConfig {
    /// Total media sessions to create, exercise, and release.
    pub total_streams: usize,
    /// Maximum media sessions retained in one batch.
    pub concurrent_streams: usize,
    /// Bidirectional audio packets processed by each session.
    pub packets_per_stream: usize,
    /// Maximum decoded frames retained in each AI-facing direction.
    pub queue_capacity: usize,
}

impl Default for MediaSmokeConfig {
    fn default() -> Self {
        Self {
            total_streams: 64,
            concurrent_streams: 8,
            packets_per_stream: 32,
            queue_capacity: 4,
        }
    }
}

/// Best-effort Linux process observations captured around a smoke run.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProcessSample {
    /// Resident bytes from `/proc/self/status`, when available.
    pub resident_bytes: Option<u64>,
    /// Open file descriptors from `/proc/self/fd`, when available.
    pub open_file_descriptors: Option<usize>,
    /// Process thread count from `/proc/self/status`, when available.
    pub threads: Option<usize>,
}

impl ProcessSample {
    pub(crate) fn capture() -> Self {
        Self {
            resident_bytes: resident_bytes(),
            open_file_descriptors: fs::read_dir("/proc/self/fd").ok().map(Iterator::count),
            threads: status_value("Threads:").and_then(|value| usize::try_from(value).ok()),
        }
    }

    pub(crate) fn include(&mut self, sample: Self) {
        self.resident_bytes = max_optional(self.resident_bytes, sample.resident_bytes);
        self.open_file_descriptors =
            max_optional(self.open_file_descriptors, sample.open_file_descriptors);
        self.threads = max_optional(self.threads, sample.threads);
    }
}

/// Deterministic counters and process observations from a media smoke run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MediaSmokeReport {
    /// Media sessions the harness attempted to exercise.
    pub attempted_streams: usize,
    /// Media sessions that completed and released all logical resources.
    pub completed_streams: usize,
    /// Media sessions that did not complete successfully.
    pub failed_streams: usize,
    /// Number of bounded batches executed.
    pub batches: usize,
    /// Highest simultaneously retained session count.
    pub peak_active_streams: usize,
    /// Valid inbound RTP audio packets accepted across all sessions.
    pub inbound_packets: u64,
    /// Jitter-buffered packets released for decoding.
    pub played_packets: u64,
    /// AI-originated audio packets serialized as RTP.
    pub outbound_packets: u64,
    /// Decoded inbound frames evicted by bounded AI backpressure.
    pub ai_queue_drops: u64,
    /// Packets rejected by jitter duplicate, late, or overflow policy.
    pub jitter_drops: u64,
    /// Highest decoded AI-facing queue depth on one session.
    pub peak_ai_queue_depth: usize,
    /// Highest jitter-buffer depth on one session.
    pub peak_jitter_depth: usize,
    /// Highest retained audio payload estimate across active sessions.
    pub peak_retained_payload_bytes: usize,
    /// Active sessions after the final batch is dropped.
    pub final_active_streams: usize,
    /// Logical media payload bytes retained after the final batch.
    pub final_retained_payload_bytes: usize,
    /// Wall time for the complete smoke run.
    pub elapsed: Duration,
    /// Process observation before allocating the first batch.
    pub process_before: ProcessSample,
    /// Highest observed process values while batches were active.
    pub process_peak: ProcessSample,
    /// Process observation after the final batch was dropped.
    pub process_after: ProcessSample,
}

/// Stage being executed when one indexed media operation failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaSmokePhase {
    /// Constructing a bounded media session.
    Create,
    /// Serializing or receiving inbound RTP.
    Receive,
    /// Releasing fixed-delay inbound audio.
    Playout,
    /// Queueing or serializing AI-originated audio.
    Send,
    /// Draining queues and checking final session bounds.
    Reclaim,
}

impl Display for MediaSmokePhase {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Create => "create",
            Self::Receive => "receive",
            Self::Playout => "playout",
            Self::Send => "send",
            Self::Reclaim => "reclaim",
        })
    }
}

/// Failure to configure or complete a deterministic media smoke run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MediaSmokeError {
    /// A configured resource bound was zero or excessive.
    InvalidConfig(&'static str),
    /// A media session rejected one indexed operation.
    Media {
        /// One-based stream number within the complete run.
        stream_number: usize,
        /// One-based packet number, or zero during construction/reclamation.
        packet_number: usize,
        /// Operation rejected by the media session.
        phase: MediaSmokePhase,
        /// Contextual media failure.
        source: MediaSessionError,
    },
    /// The harness could not serialize its known-valid RTP fixture.
    RtpSerialize {
        /// One-based stream number within the complete run.
        stream_number: usize,
        /// One-based packet number within the stream.
        packet_number: usize,
        /// Contextual RTP serialization failure.
        source: SerializeError,
    },
    /// A successful operation violated a load-harness invariant.
    Invariant {
        /// One-based stream number within the complete run.
        stream_number: usize,
        /// Stable description of the violated invariant.
        detail: &'static str,
    },
}

impl Display for MediaSmokeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfig(detail) => {
                write!(formatter, "invalid media smoke config: {detail}")
            }
            Self::Media {
                stream_number,
                packet_number,
                phase,
                source,
            } => write!(
                formatter,
                "media smoke stream {stream_number} packet {packet_number} failed during {phase}: {source}"
            ),
            Self::RtpSerialize {
                stream_number,
                packet_number,
                source,
            } => write!(
                formatter,
                "media smoke stream {stream_number} packet {packet_number} could not serialize RTP: {source}"
            ),
            Self::Invariant {
                stream_number,
                detail,
            } => write!(
                formatter,
                "media smoke stream {stream_number} violated invariant: {detail}"
            ),
        }
    }
}

impl Error for MediaSmokeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Media { source, .. } => Some(source),
            Self::RtpSerialize { source, .. } => Some(source),
            Self::InvalidConfig(_) | Self::Invariant { .. } => None,
        }
    }
}

/// Exercises bounded RTP ingress, jitter playout, AI backpressure, RTP egress,
/// and repeated media-session capacity reuse without sockets or provider access.
///
/// # Errors
///
/// Returns a contextual error for invalid bounds, media/RTP failures, or any
/// session that retains logical media resources at reclamation.
pub fn run_media_reclamation_smoke(
    config: MediaSmokeConfig,
) -> Result<MediaSmokeReport, MediaSmokeError> {
    validate_config(config)?;
    MediaSmokeRun::new().execute(config)
}

#[derive(Debug)]
struct MediaSmokeRun {
    attempted_streams: usize,
    completed_streams: usize,
    batches: usize,
    peak_active_streams: usize,
    inbound_packets: u64,
    played_packets: u64,
    outbound_packets: u64,
    ai_queue_drops: u64,
    jitter_drops: u64,
    peak_ai_queue_depth: usize,
    peak_jitter_depth: usize,
    peak_retained_payload_bytes: usize,
    process_before: ProcessSample,
    process_peak: ProcessSample,
}

impl MediaSmokeRun {
    fn new() -> Self {
        let process_before = ProcessSample::capture();
        Self {
            attempted_streams: 0,
            completed_streams: 0,
            batches: 0,
            peak_active_streams: 0,
            inbound_packets: 0,
            played_packets: 0,
            outbound_packets: 0,
            ai_queue_drops: 0,
            jitter_drops: 0,
            peak_ai_queue_depth: 0,
            peak_jitter_depth: 0,
            peak_retained_payload_bytes: 0,
            process_before,
            process_peak: process_before,
        }
    }

    fn execute(mut self, config: MediaSmokeConfig) -> Result<MediaSmokeReport, MediaSmokeError> {
        let started = Instant::now();
        while self.attempted_streams < config.total_streams {
            let batch_size = config
                .concurrent_streams
                .min(config.total_streams - self.attempted_streams);
            self.run_batch(config, batch_size)?;
        }
        let process_after = ProcessSample::capture();
        self.process_peak.include(process_after);
        Ok(MediaSmokeReport {
            attempted_streams: self.attempted_streams,
            completed_streams: self.completed_streams,
            failed_streams: self.attempted_streams - self.completed_streams,
            batches: self.batches,
            peak_active_streams: self.peak_active_streams,
            inbound_packets: self.inbound_packets,
            played_packets: self.played_packets,
            outbound_packets: self.outbound_packets,
            ai_queue_drops: self.ai_queue_drops,
            jitter_drops: self.jitter_drops,
            peak_ai_queue_depth: self.peak_ai_queue_depth,
            peak_jitter_depth: self.peak_jitter_depth,
            peak_retained_payload_bytes: self.peak_retained_payload_bytes,
            final_active_streams: 0,
            final_retained_payload_bytes: 0,
            elapsed: started.elapsed(),
            process_before: self.process_before,
            process_peak: self.process_peak,
            process_after,
        })
    }

    fn run_batch(
        &mut self,
        config: MediaSmokeConfig,
        batch_size: usize,
    ) -> Result<(), MediaSmokeError> {
        self.batches = self
            .batches
            .checked_add(1)
            .ok_or(MediaSmokeError::InvalidConfig("batch count overflowed"))?;
        let first_stream = self.attempted_streams + 1;
        let mut streams = Vec::with_capacity(batch_size);
        for offset in 0..batch_size {
            let stream_number = first_stream + offset;
            streams.push(MediaStream::new(config, stream_number)?);
        }
        self.attempted_streams += batch_size;
        self.peak_active_streams = self.peak_active_streams.max(streams.len());
        let active_streams = streams.len();
        self.process_peak.include(ProcessSample::capture());

        for packet_number in 1..=config.packets_per_stream {
            for stream in &mut streams {
                let observation = stream.process_packet(packet_number)?;
                self.inbound_packets = self.inbound_packets.saturating_add(1);
                self.played_packets = self.played_packets.saturating_add(1);
                self.outbound_packets = self.outbound_packets.saturating_add(1);
                self.peak_ai_queue_depth = self.peak_ai_queue_depth.max(observation.ai_queue_depth);
                self.peak_jitter_depth = self.peak_jitter_depth.max(observation.jitter_depth);
                self.peak_retained_payload_bytes = self
                    .peak_retained_payload_bytes
                    .max(active_streams.saturating_mul(observation.retained_payload_bytes));
            }
        }
        self.process_peak.include(ProcessSample::capture());
        for stream in &mut streams {
            let reclaimed = stream.reclaim()?;
            self.ai_queue_drops = self.ai_queue_drops.saturating_add(reclaimed.ai_queue_drops);
            self.jitter_drops = self.jitter_drops.saturating_add(reclaimed.jitter_drops);
            self.completed_streams = self.completed_streams.saturating_add(1);
        }
        streams.clear();
        self.process_peak.include(ProcessSample::capture());
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct MediaStream {
    stream_number: usize,
    media: MediaSession,
    sequence: u16,
    timestamp: u32,
    arrival: Duration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MediaObservation {
    pub(crate) ai_queue_depth: usize,
    pub(crate) jitter_depth: usize,
    pub(crate) retained_payload_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ReclaimedMedia {
    pub(crate) ai_queue_drops: u64,
    pub(crate) jitter_drops: u64,
}

impl MediaStream {
    pub(crate) fn new(
        config: MediaSmokeConfig,
        stream_number: usize,
    ) -> Result<Self, MediaSmokeError> {
        let media = MediaSession::new(
            MediaSessionConfig {
                rtp: RtpSessionConfig {
                    payload_type: 0,
                    clock_rate: 8_000,
                    remote_ssrc: Some(42),
                    max_packet_bytes: 1_024,
                    max_extension_bytes: 256,
                    ..RtpSessionConfig::default()
                },
                bridge: MediaBridgeConfig {
                    to_ai_capacity: config.queue_capacity,
                    from_ai_capacity: config.queue_capacity,
                    to_ai_policy: DropPolicy::DropOldest,
                    from_ai_policy: DropPolicy::DropOldest,
                },
                max_audio_samples: AUDIO_SAMPLES,
                jitter_buffer: Some(JitterBufferConfig {
                    max_packets: 4,
                    playout_delay: PLAYOUT_DELAY,
                }),
                ..MediaSessionConfig::default()
            },
            1,
            1_000,
        )
        .map_err(|source| MediaSmokeError::Media {
            stream_number,
            packet_number: 0,
            phase: MediaSmokePhase::Create,
            source,
        })?;
        Ok(Self {
            stream_number,
            media,
            sequence: 1,
            timestamp: 1_000,
            arrival: Duration::ZERO,
        })
    }

    pub(crate) fn process_packet(
        &mut self,
        packet_number: usize,
    ) -> Result<MediaObservation, MediaSmokeError> {
        let wire = serialize(&RtpPacket {
            padding: false,
            marker: packet_number == 1,
            payload_type: 0,
            sequence_number: self.sequence,
            timestamp: self.timestamp,
            ssrc: 42,
            csrcs: Vec::new(),
            extension: None,
            payload: vec![self.sequence.to_le_bytes()[0]; AUDIO_SAMPLES],
        })
        .map_err(|source| MediaSmokeError::RtpSerialize {
            stream_number: self.stream_number,
            packet_number,
            source,
        })?;
        let received = self
            .media
            .receive_rtp(&wire, self.arrival)
            .map_err(|source| MediaSmokeError::Media {
                stream_number: self.stream_number,
                packet_number,
                phase: MediaSmokePhase::Receive,
                source,
            })?;
        if !matches!(
            received,
            ReceivedMedia::AudioBuffered {
                outcome: JitterPushOutcome::Accepted,
                ..
            }
        ) {
            return Err(MediaSmokeError::Invariant {
                stream_number: self.stream_number,
                detail: "valid ordered RTP packet was not retained by jitter playout",
            });
        }
        let buffered_depth = self
            .media
            .stats()
            .jitter_buffer
            .map_or(0, |jitter| jitter.depth);
        let played = self
            .media
            .playout_audio(self.arrival.saturating_add(PLAYOUT_DELAY))
            .map_err(|source| MediaSmokeError::Media {
                stream_number: self.stream_number,
                packet_number,
                phase: MediaSmokePhase::Playout,
                source,
            })?;
        if !matches!(played, Some(ReceivedMedia::Audio { .. })) {
            return Err(MediaSmokeError::Invariant {
                stream_number: self.stream_number,
                detail: "retained RTP packet was not due at its fixed playout deadline",
            });
        }

        let outbound_queue = self.media.push_from_ai(AudioFrame {
            timestamp: self.timestamp,
            codec: AudioCodec::Pcmu,
            sample_rate: 8_000,
            samples: vec![i16::from(self.sequence.to_le_bytes()[0]); AUDIO_SAMPLES],
        });
        if outbound_queue != PushOutcome::Accepted {
            return Err(MediaSmokeError::Invariant {
                stream_number: self.stream_number,
                detail: "immediately drained outbound AI queue rejected audio",
            });
        }
        if self
            .media
            .next_audio_rtp(packet_number == 1)
            .map_err(|source| MediaSmokeError::Media {
                stream_number: self.stream_number,
                packet_number,
                phase: MediaSmokePhase::Send,
                source,
            })?
            .is_none()
        {
            return Err(MediaSmokeError::Invariant {
                stream_number: self.stream_number,
                detail: "queued AI audio produced no outbound RTP packet",
            });
        }

        self.sequence = self.sequence.wrapping_add(1);
        self.timestamp = self.timestamp.wrapping_add(160);
        self.arrival = self.arrival.saturating_add(PACKET_INTERVAL);
        let stats = self.media.stats();
        let jitter_depth = stats
            .jitter_buffer
            .map_or(buffered_depth, |jitter| jitter.depth.max(buffered_depth));
        let retained_frames = stats
            .bridge
            .to_ai
            .depth
            .saturating_add(stats.bridge.from_ai.depth);
        Ok(MediaObservation {
            ai_queue_depth: stats.bridge.to_ai.depth,
            jitter_depth,
            retained_payload_bytes: retained_frames
                .saturating_mul(AUDIO_SAMPLES)
                .saturating_mul(std::mem::size_of::<i16>())
                .saturating_add(buffered_depth.saturating_mul(AUDIO_SAMPLES)),
        })
    }

    pub(crate) fn reclaim(&mut self) -> Result<ReclaimedMedia, MediaSmokeError> {
        while self.media.pop_for_ai().is_some() {}
        let stats = self.media.stats();
        let jitter = stats.jitter_buffer.ok_or(MediaSmokeError::Invariant {
            stream_number: self.stream_number,
            detail: "configured jitter stats were absent",
        })?;
        if stats.bridge.to_ai.depth != 0 || stats.bridge.from_ai.depth != 0 || jitter.depth != 0 {
            return Err(MediaSmokeError::Invariant {
                stream_number: self.stream_number,
                detail: "media queues were not empty at reclamation",
            });
        }
        Ok(ReclaimedMedia {
            ai_queue_drops: stats
                .bridge
                .to_ai
                .dropped_oldest
                .saturating_add(stats.bridge.to_ai.dropped_newest),
            jitter_drops: jitter
                .dropped_duplicate
                .saturating_add(jitter.dropped_late)
                .saturating_add(jitter.dropped_overflow),
        })
    }
}

pub(crate) fn validate_config(config: MediaSmokeConfig) -> Result<(), MediaSmokeError> {
    if config.total_streams == 0 {
        return Err(MediaSmokeError::InvalidConfig(
            "total_streams must be non-zero",
        ));
    }
    if config.total_streams > MAX_STREAMS {
        return Err(MediaSmokeError::InvalidConfig(
            "total_streams exceeds the smoke safety bound",
        ));
    }
    if config.concurrent_streams == 0 {
        return Err(MediaSmokeError::InvalidConfig(
            "concurrent_streams must be non-zero",
        ));
    }
    if config.concurrent_streams > config.total_streams {
        return Err(MediaSmokeError::InvalidConfig(
            "concurrent_streams cannot exceed total_streams",
        ));
    }
    if config.packets_per_stream == 0 {
        return Err(MediaSmokeError::InvalidConfig(
            "packets_per_stream must be non-zero",
        ));
    }
    if config.packets_per_stream > MAX_PACKETS_PER_STREAM {
        return Err(MediaSmokeError::InvalidConfig(
            "packets_per_stream exceeds the smoke safety bound",
        ));
    }
    if config.queue_capacity == 0 || config.queue_capacity > 4_096 {
        return Err(MediaSmokeError::InvalidConfig(
            "queue_capacity must be between 1 and 4,096",
        ));
    }
    config
        .total_streams
        .checked_mul(config.packets_per_stream)
        .ok_or(MediaSmokeError::InvalidConfig(
            "total packet count overflows",
        ))?;
    Ok(())
}

fn resident_bytes() -> Option<u64> {
    let kibibytes = status_value("VmRSS:")?;
    kibibytes.checked_mul(1_024)
}

fn status_value(prefix: &str) -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with(prefix))?;
    line.split_whitespace().nth(1)?.parse::<u64>().ok()
}

fn max_optional<T: Ord + Copy>(left: Option<T>, right: Option<T>) -> Option<T> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_media_smoke_bounds() {
        for config in [
            MediaSmokeConfig {
                total_streams: 0,
                ..MediaSmokeConfig::default()
            },
            MediaSmokeConfig {
                total_streams: 1,
                concurrent_streams: 2,
                ..MediaSmokeConfig::default()
            },
            MediaSmokeConfig {
                packets_per_stream: 0,
                ..MediaSmokeConfig::default()
            },
            MediaSmokeConfig {
                queue_capacity: 0,
                ..MediaSmokeConfig::default()
            },
        ] {
            assert!(matches!(
                run_media_reclamation_smoke(config),
                Err(MediaSmokeError::InvalidConfig(_))
            ));
        }
    }

    #[test]
    fn reports_bidirectional_packets_backpressure_and_final_reclamation() {
        let report = run_media_reclamation_smoke(MediaSmokeConfig {
            total_streams: 5,
            concurrent_streams: 2,
            packets_per_stream: 6,
            queue_capacity: 2,
        })
        .unwrap();

        assert_eq!(report.attempted_streams, 5);
        assert_eq!(report.completed_streams, 5);
        assert_eq!(report.failed_streams, 0);
        assert_eq!(report.batches, 3);
        assert_eq!(report.peak_active_streams, 2);
        assert_eq!(report.inbound_packets, 30);
        assert_eq!(report.played_packets, 30);
        assert_eq!(report.outbound_packets, 30);
        assert_eq!(report.ai_queue_drops, 20);
        assert_eq!(report.jitter_drops, 0);
        assert_eq!(report.peak_ai_queue_depth, 2);
        assert_eq!(report.peak_jitter_depth, 1);
        assert_eq!(report.peak_retained_payload_bytes, 1_600);
        assert_eq!(report.final_active_streams, 0);
        assert_eq!(report.final_retained_payload_bytes, 0);
    }

    #[test]
    fn repeatedly_reuses_single_stream_capacity() {
        let report = run_media_reclamation_smoke(MediaSmokeConfig {
            total_streams: 128,
            concurrent_streams: 1,
            packets_per_stream: 4,
            queue_capacity: 1,
        })
        .unwrap();

        assert_eq!(report.batches, 128);
        assert_eq!(report.completed_streams, 128);
        assert_eq!(report.inbound_packets, 512);
        assert_eq!(report.outbound_packets, 512);
        assert_eq!(report.ai_queue_drops, 384);
        assert_eq!(report.final_active_streams, 0);
        assert_eq!(report.final_retained_payload_bytes, 0);
    }
}
