//! Bounded PCM recording for decoded media frames.

use std::error::Error;
use std::fmt::{Display, Formatter};

use crate::{AudioFrame, BoundedMediaQueue, DropPolicy, PushOutcome, QueueError};

/// Bounds and format settings for an [`AudioRecorder`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecorderConfig {
    /// Maximum number of decoded frames retained in memory.
    pub max_frames: usize,
    /// Maximum number of PCM samples accepted in one frame.
    pub max_samples_per_frame: usize,
    /// PCM sample rate written to the WAV header.
    pub sample_rate: u32,
    /// Number of PCM channels. The bounded recorder currently supports mono.
    pub channels: u16,
    /// Backpressure policy when the retained-frame bound is full.
    pub drop_policy: DropPolicy,
}

impl Default for RecorderConfig {
    fn default() -> Self {
        Self {
            max_frames: 9_000,
            max_samples_per_frame: 1_600,
            sample_rate: 8_000,
            channels: 1,
            drop_policy: DropPolicy::DropOldest,
        }
    }
}

impl RecorderConfig {
    fn validate(self) -> Result<Self, RecordingError> {
        if self.max_frames == 0 || self.max_samples_per_frame == 0 || self.sample_rate == 0 {
            return Err(RecordingError::InvalidConfig);
        }
        if self.channels != 1 {
            return Err(RecordingError::UnsupportedChannels(self.channels));
        }
        Ok(self)
    }
}

/// Errors raised while accepting frames or producing a recording.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecordingError {
    /// A recorder bound or sample rate was zero.
    InvalidConfig,
    /// The requested channel count is not supported by this recorder.
    UnsupportedChannels(u16),
    /// A frame has a different sample rate than the recording.
    SampleRateMismatch {
        /// Configured recording sample rate.
        expected: u32,
        /// Frame sample rate.
        actual: u32,
    },
    /// A frame exceeds the configured per-frame sample bound.
    FrameTooLarge {
        /// Number of samples supplied by the frame.
        actual: usize,
        /// Maximum accepted samples per frame.
        maximum: usize,
    },
    /// The resulting WAV cannot be represented by its 32-bit RIFF lengths.
    WaveTooLarge,
    /// The underlying bounded queue rejected its configuration.
    Queue(QueueError),
}

impl Display for RecordingError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfig => formatter.write_str("recording bounds must be non-zero"),
            Self::UnsupportedChannels(channels) => {
                write!(
                    formatter,
                    "recording supports mono only, got {channels} channels"
                )
            }
            Self::SampleRateMismatch { expected, actual } => {
                write!(
                    formatter,
                    "recording expects {expected} Hz, got {actual} Hz"
                )
            }
            Self::FrameTooLarge { actual, maximum } => {
                write!(
                    formatter,
                    "recording frame has {actual} samples, maximum is {maximum}"
                )
            }
            Self::WaveTooLarge => formatter.write_str("recording is too large for a WAV file"),
            Self::Queue(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for RecordingError {}

impl From<QueueError> for RecordingError {
    fn from(error: QueueError) -> Self {
        Self::Queue(error)
    }
}

/// Stable counters and timestamp metadata for one bounded recording.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecordingMetadata {
    /// PCM sample rate.
    pub sample_rate: u32,
    /// PCM channel count.
    pub channels: u16,
    /// RTP timestamp of the oldest retained frame, if any.
    pub first_timestamp: Option<u32>,
    /// RTP timestamp of the newest retained frame, if any.
    pub last_timestamp: Option<u32>,
    /// Number of retained frames.
    pub frames: usize,
    /// Number of retained PCM samples.
    pub samples: u64,
    /// Number of frames discarded because the bound was reached.
    pub dropped_frames: u64,
}

/// A non-blocking, bounded PCM recorder.
///
/// Callers feed already-decoded [`AudioFrame`] values from the media path.
/// The recorder only retains a configured number of bounded frames, so slow
/// persistence cannot create an unbounded queue or block RTP processing.
#[derive(Clone, Debug)]
pub struct AudioRecorder {
    config: RecorderConfig,
    frames: BoundedMediaQueue<AudioFrame>,
    first_timestamp: Option<u32>,
    last_timestamp: Option<u32>,
    samples: u64,
    dropped_frames: u64,
}

impl AudioRecorder {
    /// Creates an empty recorder with validated bounds.
    ///
    /// # Errors
    ///
    /// Returns [`RecordingError::InvalidConfig`] when a bound or format is
    /// unsupported.
    pub fn new(config: RecorderConfig) -> Result<Self, RecordingError> {
        let config = config.validate()?;
        Ok(Self {
            frames: BoundedMediaQueue::new(config.max_frames, config.drop_policy)?,
            config,
            first_timestamp: None,
            last_timestamp: None,
            samples: 0,
            dropped_frames: 0,
        })
    }

    /// Returns the immutable recorder configuration.
    #[must_use]
    pub fn config(&self) -> RecorderConfig {
        self.config
    }

    /// Accepts one decoded PCM frame without blocking.
    ///
    /// # Errors
    ///
    /// Returns an error when the frame sample rate or size exceeds the
    /// configured recording bounds. Rejected frames are not retained.
    pub fn push(&mut self, frame: AudioFrame) -> Result<PushOutcome, RecordingError> {
        if frame.sample_rate != self.config.sample_rate {
            return Err(RecordingError::SampleRateMismatch {
                expected: self.config.sample_rate,
                actual: frame.sample_rate,
            });
        }
        if frame.samples.len() > self.config.max_samples_per_frame {
            return Err(RecordingError::FrameTooLarge {
                actual: frame.samples.len(),
                maximum: self.config.max_samples_per_frame,
            });
        }

        let frame_samples = frame.samples.len() as u64;
        let oldest_samples = self
            .frames
            .front()
            .map_or(0, |oldest| oldest.samples.len() as u64);
        let timestamp = frame.timestamp;
        let outcome = self.frames.push(frame);
        match outcome {
            PushOutcome::Accepted => {
                self.samples = self.samples.saturating_add(frame_samples);
            }
            PushOutcome::DroppedOldest => {
                self.samples = self
                    .samples
                    .saturating_sub(oldest_samples)
                    .saturating_add(frame_samples);
                self.dropped_frames = self.dropped_frames.saturating_add(1);
            }
            PushOutcome::DroppedNewest => {
                self.dropped_frames = self.dropped_frames.saturating_add(1);
                return Ok(outcome);
            }
        }
        self.first_timestamp = self
            .frames
            .front()
            .map(|oldest| oldest.timestamp)
            .or(Some(timestamp));
        self.last_timestamp = Some(timestamp);
        Ok(outcome)
    }

    /// Returns metadata for the retained recording frames.
    #[must_use]
    pub fn metadata(&self) -> RecordingMetadata {
        RecordingMetadata {
            sample_rate: self.config.sample_rate,
            channels: self.config.channels,
            first_timestamp: self.first_timestamp,
            last_timestamp: self.last_timestamp,
            frames: self.frames.len(),
            samples: self.samples,
            dropped_frames: self.dropped_frames,
        }
    }

    /// Returns whether no frames are currently retained.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Serializes the retained decoded samples as a bounded PCM16 WAV file.
    ///
    /// # Errors
    ///
    /// Returns [`RecordingError::WaveTooLarge`] if the retained samples cannot
    /// be represented by the 32-bit RIFF length fields.
    pub fn wav(&self) -> Result<Vec<u8>, RecordingError> {
        let data_bytes = self
            .samples
            .checked_mul(2)
            .ok_or(RecordingError::WaveTooLarge)?;
        let data_len = usize::try_from(data_bytes).map_err(|_| RecordingError::WaveTooLarge)?;
        let riff_len = data_bytes
            .checked_add(36)
            .ok_or(RecordingError::WaveTooLarge)?;
        let data_len_u32 = u32::try_from(data_bytes).map_err(|_| RecordingError::WaveTooLarge)?;
        let riff_len_u32 = u32::try_from(riff_len).map_err(|_| RecordingError::WaveTooLarge)?;
        let byte_rate = self
            .config
            .sample_rate
            .checked_mul(u32::from(self.config.channels))
            .and_then(|rate| rate.checked_mul(2))
            .ok_or(RecordingError::WaveTooLarge)?;
        let block_align = self
            .config
            .channels
            .checked_mul(2)
            .ok_or(RecordingError::WaveTooLarge)?;
        let mut output = Vec::with_capacity(44usize.saturating_add(data_len));
        output.extend_from_slice(b"RIFF");
        output.extend_from_slice(&riff_len_u32.to_le_bytes());
        output.extend_from_slice(b"WAVEfmt ");
        output.extend_from_slice(&16u32.to_le_bytes());
        output.extend_from_slice(&1u16.to_le_bytes());
        output.extend_from_slice(&self.config.channels.to_le_bytes());
        output.extend_from_slice(&self.config.sample_rate.to_le_bytes());
        output.extend_from_slice(&byte_rate.to_le_bytes());
        output.extend_from_slice(&block_align.to_le_bytes());
        output.extend_from_slice(&16u16.to_le_bytes());
        output.extend_from_slice(b"data");
        output.extend_from_slice(&data_len_u32.to_le_bytes());
        for frame in self.frames.iter() {
            for sample in &frame.samples {
                output.extend_from_slice(&sample.to_le_bytes());
            }
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AudioCodec;

    fn frame(timestamp: u32, samples: &[i16]) -> AudioFrame {
        AudioFrame {
            timestamp,
            codec: AudioCodec::Pcmu,
            sample_rate: 8_000,
            samples: samples.to_vec(),
        }
    }

    #[test]
    fn recorder_bounds_memory_and_preserves_wav_metadata() {
        let mut recorder = AudioRecorder::new(RecorderConfig {
            max_frames: 1,
            max_samples_per_frame: 4,
            ..RecorderConfig::default()
        })
        .unwrap();
        assert_eq!(
            recorder.push(frame(100, &[1, 2])).unwrap(),
            PushOutcome::Accepted
        );
        assert_eq!(
            recorder.push(frame(200, &[3, 4, 5])).unwrap(),
            PushOutcome::DroppedOldest
        );
        assert_eq!(
            recorder.metadata(),
            RecordingMetadata {
                sample_rate: 8_000,
                channels: 1,
                first_timestamp: Some(200),
                last_timestamp: Some(200),
                frames: 1,
                samples: 3,
                dropped_frames: 1,
            }
        );
        let wav = recorder.wav().unwrap();
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 6);
        assert_eq!(&wav[44..], &[3, 0, 4, 0, 5, 0]);
    }

    #[test]
    fn invalid_frames_do_not_mutate_recording() {
        let mut recorder = AudioRecorder::new(RecorderConfig::default()).unwrap();
        assert!(matches!(
            recorder.push(AudioFrame {
                sample_rate: 16_000,
                ..frame(1, &[1])
            }),
            Err(RecordingError::SampleRateMismatch { .. })
        ));
        assert!(recorder.is_empty());
        assert!(matches!(
            recorder.push(frame(1, &[0; 1_601])),
            Err(RecordingError::FrameTooLarge { .. })
        ));
        assert!(recorder.is_empty());
    }
}
