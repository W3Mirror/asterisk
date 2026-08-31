//! Deterministic bounded WebSocket-media load and reclamation smoke testing.

use std::{
    collections::VecDeque,
    error::Error,
    fmt::{Display, Formatter},
    io::{self, Read, Write},
    time::{Duration, Instant},
};

use media_core::{
    MediaBridgeConfig, MediaSession, MediaSessionConfig, MediaSessionError, ReceivedMedia,
};
use media_websocket::{
    MediaCommand, MediaWebSocketConfig, MediaWebSocketError, MediaWebSocketEvent,
    MediaWebSocketSession, MediaWebSocketTransport, MediaWebSocketTransportConfig, OpCode,
    TransportError, WebSocketConfig, WebSocketFrame, WebSocketRole,
};
use rtp::{RtpPacket, RtpSessionConfig, SerializeError, serialize};

use crate::ProcessSample;

const AUDIO_SAMPLES: usize = 160;
const MAX_STREAMS: usize = 1_000_000;
const MAX_FRAMES_PER_STREAM: usize = 1_000_000;
const MAX_QUEUE_CAPACITY: usize = 4_096;
const MAX_FRAME_BYTES: usize = 256;
const MAX_MESSAGE_BYTES: usize = 1_024;
const MAX_CONTROL_BYTES: usize = 512;
const MAX_IDENTIFIER_BYTES: usize = 64;
const MAX_READ_BYTES: usize = 512;
const MAX_BUFFERED_BYTES: usize = 1_024;
const MAX_FRAME_WIRE_BYTES: usize = MAX_FRAME_BYTES + 14;

/// Bounds for one deterministic bidirectional WebSocket-media smoke run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WebSocketSmokeConfig {
    /// Total WebSocket media sessions to create, exercise, and release.
    pub total_streams: usize,
    /// Maximum WebSocket media sessions retained in one batch.
    pub concurrent_streams: usize,
    /// Bidirectional G.711 frames processed by each session.
    pub frames_per_stream: usize,
    /// Media and pending-write frame capacity assigned to each session.
    pub queue_capacity: usize,
}

impl Default for WebSocketSmokeConfig {
    fn default() -> Self {
        Self {
            total_streams: 64,
            concurrent_streams: 8,
            frames_per_stream: 32,
            queue_capacity: 4,
        }
    }
}

/// Deterministic counters and process observations from a WebSocket smoke run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WebSocketSmokeReport {
    /// WebSocket media sessions the harness attempted to exercise.
    pub attempted_streams: usize,
    /// WebSocket media sessions that completed and reclaimed all logical resources.
    pub completed_streams: usize,
    /// WebSocket media sessions that did not complete successfully.
    pub failed_streams: usize,
    /// Number of bounded batches executed.
    pub batches: usize,
    /// Highest simultaneously retained WebSocket media-session count.
    pub peak_active_streams: usize,
    /// Masked WebSocket audio frames accepted from the peer.
    pub inbound_websocket_frames: u64,
    /// RTP packets serialized from peer-originated WebSocket audio.
    pub outbound_rtp_packets: u64,
    /// RTP packets accepted for delivery toward the WebSocket peer.
    pub inbound_rtp_packets: u64,
    /// Unmasked WebSocket audio frames serialized and decoded at the sink.
    pub outbound_websocket_frames: u64,
    /// Full pending-write queues observed before a flush and lossless retry.
    pub write_backpressure_events: u64,
    /// Highest pending WebSocket write-frame count on one stream.
    pub peak_pending_write_frames: usize,
    /// Highest pending encoded WebSocket byte count on one stream.
    pub peak_pending_write_bytes: usize,
    /// Highest retained media queue depth on one stream.
    pub peak_media_queue_depth: usize,
    /// Active streams after the final batch is dropped.
    pub final_active_streams: usize,
    /// Pending WebSocket write frames after the final batch is dropped.
    pub final_pending_write_frames: usize,
    /// Retained media frames after the final batch is dropped.
    pub final_media_queue_depth: usize,
    /// Wall time for the complete smoke run.
    pub elapsed: Duration,
    /// Process observation before allocating the first batch.
    pub process_before: ProcessSample,
    /// Highest observed process values while batches were active.
    pub process_peak: ProcessSample,
    /// Process observation after the final batch was dropped.
    pub process_after: ProcessSample,
}

/// Stage being executed when one indexed WebSocket-media operation failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WebSocketSmokePhase {
    /// Constructing the media adapter and bounded transport.
    Create,
    /// Encoding or receiving a peer WebSocket frame.
    ReceiveWebSocket,
    /// Serializing peer-originated audio as RTP.
    SendRtp,
    /// Serializing or receiving RTP for WebSocket delivery.
    ReceiveRtp,
    /// Queueing, flushing, or decoding WebSocket output.
    SendWebSocket,
    /// Draining queues and checking final stream bounds.
    Reclaim,
}

impl Display for WebSocketSmokePhase {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Create => "create",
            Self::ReceiveWebSocket => "receive WebSocket",
            Self::SendRtp => "send RTP",
            Self::ReceiveRtp => "receive RTP",
            Self::SendWebSocket => "send WebSocket",
            Self::Reclaim => "reclaim",
        })
    }
}

/// Failure to configure or complete a deterministic WebSocket-media smoke run.
#[derive(Debug)]
pub enum WebSocketSmokeError {
    /// A configured resource bound was zero or excessive.
    InvalidConfig(&'static str),
    /// A media-session operation failed.
    Media {
        /// One-based stream number within the complete run.
        stream_number: usize,
        /// One-based frame number, or zero during setup/reclamation.
        frame_number: usize,
        /// Operation rejected by the media session.
        phase: WebSocketSmokePhase,
        /// Contextual media failure.
        source: MediaSessionError,
    },
    /// A WebSocket adapter operation failed.
    WebSocket {
        /// One-based stream number within the complete run.
        stream_number: usize,
        /// One-based frame number, or zero during setup/reclamation.
        frame_number: usize,
        /// Operation rejected by the WebSocket adapter.
        phase: WebSocketSmokePhase,
        /// Contextual adapter failure.
        source: MediaWebSocketError,
    },
    /// A bounded transport operation failed.
    Transport {
        /// One-based stream number within the complete run.
        stream_number: usize,
        /// One-based frame number, or zero during setup/reclamation.
        frame_number: usize,
        /// Operation rejected by the transport.
        phase: WebSocketSmokePhase,
        /// Contextual transport failure.
        source: TransportError,
    },
    /// A known-valid RTP fixture could not be serialized.
    RtpSerialize {
        /// One-based stream number within the complete run.
        stream_number: usize,
        /// One-based frame number within the stream.
        frame_number: usize,
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

impl Display for WebSocketSmokeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfig(detail) => {
                write!(formatter, "invalid WebSocket smoke config: {detail}")
            }
            Self::Media {
                stream_number,
                frame_number,
                phase,
                source,
            } => write!(
                formatter,
                "WebSocket smoke stream {stream_number} frame {frame_number} failed during {phase}: {source}"
            ),
            Self::WebSocket {
                stream_number,
                frame_number,
                phase,
                source,
            } => write!(
                formatter,
                "WebSocket smoke stream {stream_number} frame {frame_number} failed during {phase}: {source}"
            ),
            Self::Transport {
                stream_number,
                frame_number,
                phase,
                source,
            } => write!(
                formatter,
                "WebSocket smoke stream {stream_number} frame {frame_number} failed during {phase}: {source}"
            ),
            Self::RtpSerialize {
                stream_number,
                frame_number,
                source,
            } => write!(
                formatter,
                "WebSocket smoke stream {stream_number} frame {frame_number} could not serialize RTP: {source}"
            ),
            Self::Invariant {
                stream_number,
                detail,
            } => write!(
                formatter,
                "WebSocket smoke stream {stream_number} violated invariant: {detail}"
            ),
        }
    }
}

impl Error for WebSocketSmokeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Media { source, .. } => Some(source),
            Self::WebSocket { source, .. } => Some(source),
            Self::Transport { source, .. } => Some(source),
            Self::RtpSerialize { source, .. } => Some(source),
            Self::InvalidConfig(_) | Self::Invariant { .. } => None,
        }
    }
}

/// Exercises bounded WebSocket parsing, media bridging, write backpressure,
/// partial writes, and repeated transport capacity reuse without sockets.
///
/// # Errors
///
/// Returns a contextual error for invalid bounds, media/protocol failures, or
/// any stream that retains logical media or transport resources at reclamation.
pub fn run_websocket_reclamation_smoke(
    config: WebSocketSmokeConfig,
) -> Result<WebSocketSmokeReport, WebSocketSmokeError> {
    validate_config(config)?;
    WebSocketSmokeRun::new().execute(config)
}

#[derive(Debug)]
struct WebSocketSmokeRun {
    attempted_streams: usize,
    completed_streams: usize,
    batches: usize,
    peak_active_streams: usize,
    inbound_websocket_frames: u64,
    outbound_rtp_packets: u64,
    inbound_rtp_packets: u64,
    outbound_websocket_frames: u64,
    write_backpressure_events: u64,
    peak_pending_write_frames: usize,
    peak_pending_write_bytes: usize,
    peak_media_queue_depth: usize,
    process_before: ProcessSample,
    process_peak: ProcessSample,
}

impl WebSocketSmokeRun {
    fn new() -> Self {
        let process_before = ProcessSample::capture();
        Self {
            attempted_streams: 0,
            completed_streams: 0,
            batches: 0,
            peak_active_streams: 0,
            inbound_websocket_frames: 0,
            outbound_rtp_packets: 0,
            inbound_rtp_packets: 0,
            outbound_websocket_frames: 0,
            write_backpressure_events: 0,
            peak_pending_write_frames: 0,
            peak_pending_write_bytes: 0,
            peak_media_queue_depth: 0,
            process_before,
            process_peak: process_before,
        }
    }

    fn execute(
        mut self,
        config: WebSocketSmokeConfig,
    ) -> Result<WebSocketSmokeReport, WebSocketSmokeError> {
        let started = Instant::now();
        while self.attempted_streams < config.total_streams {
            let batch_size = config
                .concurrent_streams
                .min(config.total_streams - self.attempted_streams);
            self.run_batch(config, batch_size)?;
        }
        let process_after = ProcessSample::capture();
        self.process_peak.include(process_after);
        Ok(WebSocketSmokeReport {
            attempted_streams: self.attempted_streams,
            completed_streams: self.completed_streams,
            failed_streams: self.attempted_streams - self.completed_streams,
            batches: self.batches,
            peak_active_streams: self.peak_active_streams,
            inbound_websocket_frames: self.inbound_websocket_frames,
            outbound_rtp_packets: self.outbound_rtp_packets,
            inbound_rtp_packets: self.inbound_rtp_packets,
            outbound_websocket_frames: self.outbound_websocket_frames,
            write_backpressure_events: self.write_backpressure_events,
            peak_pending_write_frames: self.peak_pending_write_frames,
            peak_pending_write_bytes: self.peak_pending_write_bytes,
            peak_media_queue_depth: self.peak_media_queue_depth,
            final_active_streams: 0,
            final_pending_write_frames: 0,
            final_media_queue_depth: 0,
            elapsed: started.elapsed(),
            process_before: self.process_before,
            process_peak: self.process_peak,
            process_after,
        })
    }

    fn run_batch(
        &mut self,
        config: WebSocketSmokeConfig,
        batch_size: usize,
    ) -> Result<(), WebSocketSmokeError> {
        self.batches = self
            .batches
            .checked_add(1)
            .ok_or(WebSocketSmokeError::InvalidConfig("batch count overflowed"))?;
        let first_stream = self.attempted_streams + 1;
        let mut streams = Vec::with_capacity(batch_size);
        for offset in 0..batch_size {
            streams.push(WebSocketStream::new(config, first_stream + offset)?);
        }
        self.attempted_streams += batch_size;
        self.peak_active_streams = self.peak_active_streams.max(streams.len());
        self.process_peak.include(ProcessSample::capture());

        for frame_number in 1..=config.frames_per_stream {
            for stream in &mut streams {
                let observation = stream.process_frame(frame_number)?;
                self.inbound_websocket_frames = self.inbound_websocket_frames.saturating_add(1);
                self.outbound_rtp_packets = self.outbound_rtp_packets.saturating_add(1);
                self.inbound_rtp_packets = self.inbound_rtp_packets.saturating_add(1);
                self.write_backpressure_events = self
                    .write_backpressure_events
                    .saturating_add(u64::from(observation.write_backpressure));
                self.peak_pending_write_frames = self
                    .peak_pending_write_frames
                    .max(observation.pending_write_frames);
                self.peak_pending_write_bytes = self
                    .peak_pending_write_bytes
                    .max(observation.pending_write_bytes);
                self.peak_media_queue_depth = self
                    .peak_media_queue_depth
                    .max(observation.media_queue_depth);
            }
        }
        self.process_peak.include(ProcessSample::capture());
        for stream in streams {
            let reclaimed = stream.reclaim(config.frames_per_stream)?;
            self.outbound_websocket_frames = self
                .outbound_websocket_frames
                .saturating_add(reclaimed.outbound_websocket_frames);
            self.completed_streams = self.completed_streams.saturating_add(1);
        }
        self.process_peak.include(ProcessSample::capture());
        Ok(())
    }
}

struct WebSocketStream {
    stream_number: usize,
    transport: MediaWebSocketTransport<SmokeStream>,
    sequence: u16,
    timestamp: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct WebSocketObservation {
    write_backpressure: bool,
    pending_write_frames: usize,
    pending_write_bytes: usize,
    media_queue_depth: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ReclaimedWebSocket {
    outbound_websocket_frames: u64,
}

impl WebSocketStream {
    fn new(
        config: WebSocketSmokeConfig,
        stream_number: usize,
    ) -> Result<Self, WebSocketSmokeError> {
        let adapter_config = adapter_config(WebSocketRole::Server);
        let adapter = MediaWebSocketSession::new(adapter_config).map_err(|source| {
            WebSocketSmokeError::WebSocket {
                stream_number,
                frame_number: 0,
                phase: WebSocketSmokePhase::Create,
                source,
            }
        })?;
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
                    ..MediaBridgeConfig::default()
                },
                max_audio_samples: AUDIO_SAMPLES,
                ..MediaSessionConfig::default()
            },
            1,
            1_000,
        )
        .map_err(|source| WebSocketSmokeError::Media {
            stream_number,
            frame_number: 0,
            phase: WebSocketSmokePhase::Create,
            source,
        })?;
        let mut stream = SmokeStream::default();
        stream.push_input(peer_frame(
            &WebSocketFrame::text(format!(
                "MEDIA_START connection_id:stream-{stream_number} channel_id:channel-{stream_number} format:ulaw optimal_frame_size:{AUDIO_SAMPLES} sample_rate:8000 direction:both"
            )),
            stream_number,
            0,
        )?);
        let mut transport = MediaWebSocketTransport::new(
            stream,
            adapter,
            media,
            MediaWebSocketTransportConfig {
                max_read_bytes: MAX_READ_BYTES,
                max_buffered_bytes: MAX_BUFFERED_BYTES,
                max_pending_writes: config.queue_capacity,
                max_pending_write_bytes: config
                    .queue_capacity
                    .checked_mul(MAX_FRAME_WIRE_BYTES)
                    .ok_or(WebSocketSmokeError::InvalidConfig(
                        "pending write bytes overflowed",
                    ))?,
            },
        )
        .map_err(|source| WebSocketSmokeError::Transport {
            stream_number,
            frame_number: 0,
            phase: WebSocketSmokePhase::Create,
            source,
        })?;
        let events = transport
            .read_once()
            .map_err(|source| WebSocketSmokeError::Transport {
                stream_number,
                frame_number: 0,
                phase: WebSocketSmokePhase::ReceiveWebSocket,
                source,
            })?;
        if !matches!(
            events.as_slice(),
            [MediaWebSocketEvent::Command(MediaCommand::Start(_))]
        ) {
            return Err(WebSocketSmokeError::Invariant {
                stream_number,
                detail: "MEDIA_START did not produce exactly one start event",
            });
        }
        Ok(Self {
            stream_number,
            transport,
            sequence: 1,
            timestamp: 1_000,
        })
    }

    fn process_frame(
        &mut self,
        frame_number: usize,
    ) -> Result<WebSocketObservation, WebSocketSmokeError> {
        self.receive_peer_audio(frame_number)?;
        self.receive_rtp_audio(frame_number)?;
        let write_backpressure = self.queue_websocket_audio(frame_number)?;

        let pending_write_frames = self.transport.pending_write_frames();
        let pending_write_bytes = self.transport.pending_write_bytes();
        let stats = self.transport.media().stats();
        let media_queue_depth = stats
            .bridge
            .to_ai
            .depth
            .saturating_add(stats.bridge.from_ai.depth);
        self.sequence = self.sequence.wrapping_add(1);
        self.timestamp = self.timestamp.wrapping_add(160);
        Ok(WebSocketObservation {
            write_backpressure,
            pending_write_frames,
            pending_write_bytes,
            media_queue_depth,
        })
    }

    fn receive_peer_audio(&mut self, frame_number: usize) -> Result<(), WebSocketSmokeError> {
        self.transport.stream_mut().push_input(peer_frame(
            &WebSocketFrame::binary(vec![self.sequence.to_le_bytes()[0]; AUDIO_SAMPLES]),
            self.stream_number,
            frame_number,
        )?);
        let events =
            self.transport
                .read_once()
                .map_err(|source| WebSocketSmokeError::Transport {
                    stream_number: self.stream_number,
                    frame_number,
                    phase: WebSocketSmokePhase::ReceiveWebSocket,
                    source,
                })?;
        if !matches!(
            events.as_slice(),
            [MediaWebSocketEvent::Audio {
                samples: AUDIO_SAMPLES,
                ..
            }]
        ) {
            return Err(WebSocketSmokeError::Invariant {
                stream_number: self.stream_number,
                detail: "peer WebSocket audio did not produce exactly one media frame",
            });
        }
        if self
            .transport
            .media_mut()
            .next_audio_rtp(frame_number == 1)
            .map_err(|source| WebSocketSmokeError::Media {
                stream_number: self.stream_number,
                frame_number,
                phase: WebSocketSmokePhase::SendRtp,
                source,
            })?
            .is_none()
        {
            return Err(WebSocketSmokeError::Invariant {
                stream_number: self.stream_number,
                detail: "peer WebSocket audio produced no outbound RTP packet",
            });
        }
        Ok(())
    }

    fn receive_rtp_audio(&mut self, frame_number: usize) -> Result<(), WebSocketSmokeError> {
        let rtp_wire = serialize(&RtpPacket {
            padding: false,
            marker: frame_number == 1,
            payload_type: 0,
            sequence_number: self.sequence,
            timestamp: self.timestamp,
            ssrc: 42,
            csrcs: Vec::new(),
            extension: None,
            payload: vec![self.sequence.to_le_bytes()[0]; AUDIO_SAMPLES],
        })
        .map_err(|source| WebSocketSmokeError::RtpSerialize {
            stream_number: self.stream_number,
            frame_number,
            source,
        })?;
        if !matches!(
            self.transport
                .media_mut()
                .receive_rtp(&rtp_wire, Duration::ZERO)
                .map_err(|source| WebSocketSmokeError::Media {
                    stream_number: self.stream_number,
                    frame_number,
                    phase: WebSocketSmokePhase::ReceiveRtp,
                    source,
                })?,
            ReceivedMedia::Audio { .. }
        ) {
            return Err(WebSocketSmokeError::Invariant {
                stream_number: self.stream_number,
                detail: "valid RTP did not produce WebSocket-bound audio",
            });
        }
        Ok(())
    }

    fn queue_websocket_audio(&mut self, frame_number: usize) -> Result<bool, WebSocketSmokeError> {
        match self.transport.queue_audio() {
            Ok(true) => Ok(false),
            Err(TransportError::WriteQueueFull { .. }) => {
                if self.transport.media().stats().bridge.to_ai.depth != 1 {
                    return Err(WebSocketSmokeError::Invariant {
                        stream_number: self.stream_number,
                        detail: "full write queue did not preserve the WebSocket-bound audio",
                    });
                }
                self.flush(frame_number)?;
                if !self.transport.queue_audio().map_err(|source| {
                    WebSocketSmokeError::Transport {
                        stream_number: self.stream_number,
                        frame_number,
                        phase: WebSocketSmokePhase::SendWebSocket,
                        source,
                    }
                })? {
                    return Err(WebSocketSmokeError::Invariant {
                        stream_number: self.stream_number,
                        detail: "preserved audio was absent after write-queue retry",
                    });
                }
                Ok(true)
            }
            Ok(false) => Err(WebSocketSmokeError::Invariant {
                stream_number: self.stream_number,
                detail: "RTP audio produced no WebSocket output frame",
            }),
            Err(source) => Err(WebSocketSmokeError::Transport {
                stream_number: self.stream_number,
                frame_number,
                phase: WebSocketSmokePhase::SendWebSocket,
                source,
            }),
        }
    }

    fn flush(&mut self, frame_number: usize) -> Result<(), WebSocketSmokeError> {
        self.transport
            .flush()
            .map_err(|source| WebSocketSmokeError::Transport {
                stream_number: self.stream_number,
                frame_number,
                phase: WebSocketSmokePhase::SendWebSocket,
                source,
            })?;
        Ok(())
    }

    fn reclaim(
        mut self,
        expected_frames: usize,
    ) -> Result<ReclaimedWebSocket, WebSocketSmokeError> {
        self.flush(0)?;
        if self.transport.pending_write_frames() != 0 || self.transport.pending_write_bytes() != 0 {
            return Err(WebSocketSmokeError::Invariant {
                stream_number: self.stream_number,
                detail: "transport retained pending writes at reclamation",
            });
        }
        let stats = self.transport.media().stats();
        if stats.bridge.to_ai.depth != 0 || stats.bridge.from_ai.depth != 0 {
            return Err(WebSocketSmokeError::Invariant {
                stream_number: self.stream_number,
                detail: "media queues were not empty at reclamation",
            });
        }
        let (stream, adapter, _media) = self.transport.into_parts();
        if adapter.stream().is_none() {
            return Err(WebSocketSmokeError::Invariant {
                stream_number: self.stream_number,
                detail: "active WebSocket metadata disappeared before reclamation",
            });
        }
        let outbound_websocket_frames =
            decode_output_frames(&stream.output, self.stream_number, expected_frames)?;
        Ok(ReclaimedWebSocket {
            outbound_websocket_frames,
        })
    }
}

#[derive(Debug, Default)]
struct SmokeStream {
    input: VecDeque<u8>,
    output: Vec<u8>,
}

impl SmokeStream {
    fn push_input(&mut self, bytes: Vec<u8>) {
        self.input.extend(bytes);
    }
}

impl Read for SmokeStream {
    fn read(&mut self, target: &mut [u8]) -> io::Result<usize> {
        let amount = target.len().min(self.input.len());
        for destination in &mut target[..amount] {
            *destination = self.input.pop_front().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "input queue changed during read",
                )
            })?;
        }
        Ok(amount)
    }
}

impl Write for SmokeStream {
    fn write(&mut self, source: &[u8]) -> io::Result<usize> {
        let amount = source.len().min(37);
        self.output.extend_from_slice(&source[..amount]);
        Ok(amount)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn adapter_config(role: WebSocketRole) -> MediaWebSocketConfig {
    MediaWebSocketConfig {
        websocket: WebSocketConfig {
            role,
            max_frame_bytes: MAX_FRAME_BYTES,
            max_message_bytes: MAX_MESSAGE_BYTES,
            max_fragments: 4,
        },
        max_control_bytes: MAX_CONTROL_BYTES,
        max_identifier_bytes: MAX_IDENTIFIER_BYTES,
        max_frame_samples: AUDIO_SAMPLES,
    }
}

fn peer_frame(
    frame: &WebSocketFrame,
    stream_number: usize,
    frame_number: usize,
) -> Result<Vec<u8>, WebSocketSmokeError> {
    frame
        .encode(
            adapter_config(WebSocketRole::Client).websocket,
            Some(mask_key(stream_number, frame_number)),
        )
        .map_err(|source| WebSocketSmokeError::WebSocket {
            stream_number,
            frame_number,
            phase: WebSocketSmokePhase::ReceiveWebSocket,
            source: MediaWebSocketError::WebSocket(source),
        })
}

fn mask_key(stream_number: usize, frame_number: usize) -> [u8; 4] {
    let stream = stream_number.to_le_bytes();
    let frame = frame_number.to_le_bytes();
    [stream[0], stream[1], frame[0], frame[1]]
}

fn decode_output_frames(
    mut output: &[u8],
    stream_number: usize,
    expected_frames: usize,
) -> Result<u64, WebSocketSmokeError> {
    let mut decoded = 0usize;
    while !output.is_empty() {
        let (frame, consumed) =
            WebSocketFrame::decode(output, adapter_config(WebSocketRole::Client).websocket)
                .map_err(|source| WebSocketSmokeError::WebSocket {
                    stream_number,
                    frame_number: decoded.saturating_add(1),
                    phase: WebSocketSmokePhase::Reclaim,
                    source: MediaWebSocketError::WebSocket(source),
                })?;
        if frame.opcode != OpCode::Binary || frame.payload.len() != AUDIO_SAMPLES {
            return Err(WebSocketSmokeError::Invariant {
                stream_number,
                detail: "WebSocket output was not one complete G.711 audio frame",
            });
        }
        output = &output[consumed..];
        decoded = decoded.saturating_add(1);
    }
    if decoded != expected_frames {
        return Err(WebSocketSmokeError::Invariant {
            stream_number,
            detail: "WebSocket output frame count did not match input RTP count",
        });
    }
    u64::try_from(decoded).map_err(|_| WebSocketSmokeError::InvalidConfig("frame count overflowed"))
}

fn validate_config(config: WebSocketSmokeConfig) -> Result<(), WebSocketSmokeError> {
    if config.total_streams == 0 {
        return Err(WebSocketSmokeError::InvalidConfig(
            "total_streams must be non-zero",
        ));
    }
    if config.total_streams > MAX_STREAMS {
        return Err(WebSocketSmokeError::InvalidConfig(
            "total_streams exceeds the smoke safety bound",
        ));
    }
    if config.concurrent_streams == 0 || config.concurrent_streams > config.total_streams {
        return Err(WebSocketSmokeError::InvalidConfig(
            "concurrent_streams must be between 1 and total_streams",
        ));
    }
    if config.frames_per_stream == 0 || config.frames_per_stream > MAX_FRAMES_PER_STREAM {
        return Err(WebSocketSmokeError::InvalidConfig(
            "frames_per_stream must be within the smoke safety bound",
        ));
    }
    if config.queue_capacity == 0 || config.queue_capacity > MAX_QUEUE_CAPACITY {
        return Err(WebSocketSmokeError::InvalidConfig(
            "queue_capacity must be between 1 and 4,096",
        ));
    }
    config
        .total_streams
        .checked_mul(config.frames_per_stream)
        .ok_or(WebSocketSmokeError::InvalidConfig(
            "total frame count overflows",
        ))?;
    config
        .queue_capacity
        .checked_mul(MAX_FRAME_WIRE_BYTES)
        .ok_or(WebSocketSmokeError::InvalidConfig(
            "pending write bytes overflow",
        ))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_websocket_smoke_bounds() {
        for config in [
            WebSocketSmokeConfig {
                total_streams: 0,
                ..WebSocketSmokeConfig::default()
            },
            WebSocketSmokeConfig {
                total_streams: 1,
                concurrent_streams: 2,
                ..WebSocketSmokeConfig::default()
            },
            WebSocketSmokeConfig {
                frames_per_stream: 0,
                ..WebSocketSmokeConfig::default()
            },
            WebSocketSmokeConfig {
                queue_capacity: 0,
                ..WebSocketSmokeConfig::default()
            },
        ] {
            assert!(matches!(
                run_websocket_reclamation_smoke(config),
                Err(WebSocketSmokeError::InvalidConfig(_))
            ));
        }
    }

    #[test]
    fn reports_bidirectional_frames_backpressure_and_final_reclamation() {
        let report = run_websocket_reclamation_smoke(WebSocketSmokeConfig {
            total_streams: 5,
            concurrent_streams: 2,
            frames_per_stream: 6,
            queue_capacity: 2,
        })
        .unwrap();

        assert_eq!(report.attempted_streams, 5);
        assert_eq!(report.completed_streams, 5);
        assert_eq!(report.failed_streams, 0);
        assert_eq!(report.batches, 3);
        assert_eq!(report.peak_active_streams, 2);
        assert_eq!(report.inbound_websocket_frames, 30);
        assert_eq!(report.outbound_rtp_packets, 30);
        assert_eq!(report.inbound_rtp_packets, 30);
        assert_eq!(report.outbound_websocket_frames, 30);
        assert_eq!(report.write_backpressure_events, 10);
        assert_eq!(report.peak_pending_write_frames, 2);
        assert!(report.peak_pending_write_bytes > AUDIO_SAMPLES);
        assert_eq!(report.peak_media_queue_depth, 0);
        assert_eq!(report.final_active_streams, 0);
        assert_eq!(report.final_pending_write_frames, 0);
        assert_eq!(report.final_media_queue_depth, 0);
    }

    #[test]
    fn repeatedly_reuses_single_websocket_stream_capacity() {
        let report = run_websocket_reclamation_smoke(WebSocketSmokeConfig {
            total_streams: 128,
            concurrent_streams: 1,
            frames_per_stream: 4,
            queue_capacity: 1,
        })
        .unwrap();

        assert_eq!(report.batches, 128);
        assert_eq!(report.completed_streams, 128);
        assert_eq!(report.inbound_websocket_frames, 512);
        assert_eq!(report.outbound_rtp_packets, 512);
        assert_eq!(report.inbound_rtp_packets, 512);
        assert_eq!(report.outbound_websocket_frames, 512);
        assert_eq!(report.write_backpressure_events, 384);
        assert_eq!(report.final_active_streams, 0);
        assert_eq!(report.final_pending_write_frames, 0);
        assert_eq!(report.final_media_queue_depth, 0);
    }

    #[test]
    fn ai_disconnect_reclaims_websocket_buffers_and_media_queues() {
        let mut stream = WebSocketStream::new(
            WebSocketSmokeConfig {
                total_streams: 1,
                concurrent_streams: 1,
                frames_per_stream: 1,
                queue_capacity: 2,
            },
            1,
        )
        .unwrap();
        stream
            .transport
            .media_mut()
            .push_from_ai(media_core::AudioFrame {
                timestamp: 1,
                codec: media_core::AudioCodec::Pcmu,
                sample_rate: 8_000,
                samples: vec![0; AUDIO_SAMPLES],
            });
        stream
            .transport
            .queue_command(&MediaCommand::Answer)
            .unwrap();
        stream.transport.stream_mut().push_input(vec![0x82]);
        assert!(stream.transport.read_once().unwrap().is_empty());
        assert!(matches!(
            stream.transport.read_once(),
            Err(TransportError::ConnectionClosed { buffered_bytes: 1 })
        ));

        let cleanup = stream.transport.cleanup_after_failure();
        assert_eq!(cleanup.buffered_read_bytes, 1);
        assert_eq!(cleanup.pending_write_frames, 1);
        assert_eq!(cleanup.media.from_ai_frames, 1);
        assert!(stream.transport.is_failed());
        assert_eq!(stream.transport.pending_write_frames(), 0);
        assert_eq!(stream.transport.media().stats().bridge.from_ai.depth, 0);
        assert!(stream.transport.adapter().stream().is_none());
    }
}
