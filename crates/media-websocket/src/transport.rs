//! Bounded blocking I/O for an already-upgraded media WebSocket.

use std::{
    collections::VecDeque,
    error::Error,
    fmt::{self, Display, Formatter},
    fs::File,
    io::{self, Read, Write},
};

use media_core::{MediaReclamation, MediaSession};

use crate::{
    CloseFrame, MediaCommand, MediaStart, MediaWebSocketError, MediaWebSocketEvent,
    MediaWebSocketSession, OpCode, WebSocketFrame, WebSocketRole,
};

const MAX_FRAME_WIRE_OVERHEAD: usize = 14;
const DEFAULT_MAX_READ_BYTES: usize = 16 * 1024;
const DEFAULT_MAX_BUFFERED_BYTES: usize = 512 * 1024;
const DEFAULT_MAX_PENDING_WRITES: usize = 64;
const DEFAULT_MAX_PENDING_WRITE_BYTES: usize = 512 * 1024;

/// Supplies fresh masking keys for client WebSocket frames.
///
/// RFC 6455 requires each client-to-server frame to use a new, unpredictable
/// key. Server-side media connections do not need a source because server
/// frames are unmasked. Implementations should obtain keys from an operating
/// system random source rather than deriving them from media or call state.
pub trait MaskKeySource {
    /// Returns one masking key for the next client frame.
    fn next_mask_key(&mut self) -> Result<[u8; 4], MaskKeyError>;
}

/// Masking-key source backed by the operating system's random device.
///
/// This implementation targets the Linux deployment environment. It opens
/// `/dev/urandom` once and reads four fresh bytes for every client frame.
#[derive(Debug)]
pub struct OsRandomMaskKeySource {
    source: File,
}

impl OsRandomMaskKeySource {
    /// Opens the operating-system random source.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when `/dev/urandom` cannot be opened.
    pub fn new() -> Result<Self, io::Error> {
        Ok(Self {
            source: File::open("/dev/urandom")?,
        })
    }
}

impl MaskKeySource for OsRandomMaskKeySource {
    fn next_mask_key(&mut self) -> Result<[u8; 4], MaskKeyError> {
        let mut key = [0; 4];
        self.source.read_exact(&mut key).map_err(MaskKeyError::Io)?;
        Ok(key)
    }
}

/// Errors returned while obtaining a WebSocket masking key.
#[derive(Debug)]
pub enum MaskKeyError {
    /// No masking-key source was configured for a client endpoint.
    Unavailable,
    /// The operating-system or application random source failed.
    Io(io::Error),
}

impl Display for MaskKeyError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => formatter.write_str("WebSocket masking-key source unavailable"),
            Self::Io(error) => write!(formatter, "WebSocket masking-key source failed: {error}"),
        }
    }
}

impl Error for MaskKeyError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Unavailable => None,
        }
    }
}

/// The default source used by [`MediaWebSocketTransport::new`].
///
/// It deliberately fails for client-role transports instead of generating
/// predictable masking keys. Applications that act as WebSocket clients must
/// use [`MediaWebSocketTransport::with_mask_source`] with an OS-backed source.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoMaskKeySource;

impl MaskKeySource for NoMaskKeySource {
    fn next_mask_key(&mut self) -> Result<[u8; 4], MaskKeyError> {
        Err(MaskKeyError::Unavailable)
    }
}

/// Resource bounds for one blocking media WebSocket stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MediaWebSocketTransportConfig {
    /// Maximum bytes requested from the underlying stream in one read.
    pub max_read_bytes: usize,
    /// Maximum encoded WebSocket bytes retained while a frame is incomplete.
    pub max_buffered_bytes: usize,
    /// Maximum number of encoded frames waiting to be written.
    pub max_pending_writes: usize,
    /// Maximum encoded bytes waiting to be written.
    pub max_pending_write_bytes: usize,
}

impl Default for MediaWebSocketTransportConfig {
    fn default() -> Self {
        Self {
            max_read_bytes: DEFAULT_MAX_READ_BYTES,
            max_buffered_bytes: DEFAULT_MAX_BUFFERED_BYTES,
            max_pending_writes: DEFAULT_MAX_PENDING_WRITES,
            max_pending_write_bytes: DEFAULT_MAX_PENDING_WRITE_BYTES,
        }
    }
}

impl MediaWebSocketTransportConfig {
    fn validate(self, websocket: &MediaWebSocketSession) -> Result<Self, TransportError> {
        if self.max_read_bytes == 0
            || self.max_buffered_bytes == 0
            || self.max_pending_writes == 0
            || self.max_pending_write_bytes == 0
        {
            return Err(TransportError::InvalidConfig);
        }
        let maximum_wire = websocket
            .config()
            .websocket
            .max_frame_bytes
            .checked_add(MAX_FRAME_WIRE_OVERHEAD)
            .ok_or(TransportError::InvalidConfig)?;
        if self.max_buffered_bytes < maximum_wire || self.max_pending_write_bytes < maximum_wire {
            return Err(TransportError::InvalidConfig);
        }
        Ok(self)
    }
}

/// Errors raised while driving a bounded media WebSocket stream.
#[derive(Debug)]
pub enum TransportError {
    /// One of the stream or adapter bounds was invalid.
    InvalidConfig,
    /// The underlying stream returned an I/O error.
    Io(io::Error),
    /// The media adapter rejected an incoming or outgoing message.
    Media(MediaWebSocketError),
    /// The stream reached EOF; the field reports retained partial bytes.
    ConnectionClosed { buffered_bytes: usize },
    /// A read would exceed the configured incomplete-frame bound.
    ReadBufferFull { actual: usize, maximum: usize },
    /// The outbound frame queue has reached one of its configured bounds.
    WriteQueueFull {
        frames: usize,
        bytes: usize,
        maximum_frames: usize,
        maximum_bytes: usize,
    },
    /// A write returned zero bytes without reporting an error.
    WriteZero,
    /// The WebSocket close handshake has started or the stream is closed.
    Closed,
    /// A client-role stream needs a masking-key source.
    MaskKey(MaskKeyError),
}

/// Bounded state released after an unrecoverable media WebSocket failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaWebSocketCleanup {
    /// Negotiated stream metadata that was detached from the adapter.
    pub stream: Option<MediaStart>,
    /// Partial encoded bytes removed from the inbound read buffer.
    pub buffered_read_bytes: usize,
    /// Encoded frames discarded from the outbound write queue.
    pub pending_write_frames: usize,
    /// Encoded bytes discarded from the outbound write queue.
    pub pending_write_bytes: usize,
    /// Media queue, jitter, and DTMF items reclaimed from the session.
    pub media: MediaReclamation,
}

impl Display for TransportError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig => {
                formatter.write_str("media WebSocket transport bounds are invalid")
            }
            Self::Io(error) => Display::fmt(error, formatter),
            Self::Media(error) => Display::fmt(error, formatter),
            Self::ConnectionClosed { buffered_bytes } => {
                write!(
                    formatter,
                    "media WebSocket stream closed with {buffered_bytes} buffered bytes"
                )
            }
            Self::ReadBufferFull { actual, maximum } => {
                write!(
                    formatter,
                    "media WebSocket read buffer has {actual} bytes, maximum is {maximum}"
                )
            }
            Self::WriteQueueFull {
                frames,
                bytes,
                maximum_frames,
                maximum_bytes,
            } => write!(
                formatter,
                "media WebSocket write queue has {frames} frames/{bytes} bytes, maximum is {maximum_frames} frames/{maximum_bytes} bytes"
            ),
            Self::WriteZero => {
                formatter.write_str("media WebSocket stream write returned zero bytes")
            }
            Self::Closed => formatter.write_str("media WebSocket stream is closed"),
            Self::MaskKey(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for TransportError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Media(error) => Some(error),
            Self::MaskKey(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for TransportError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<MediaWebSocketError> for TransportError {
    fn from(error: MediaWebSocketError) -> Self {
        Self::Media(error)
    }
}

/// A bounded blocking stream driver for [`MediaWebSocketSession`].
///
/// The stream must already have completed its HTTP/WebSocket upgrade. This
/// type owns incremental read buffering and a bounded write queue, drives the
/// adapter against one [`MediaSession`], automatically queues pong replies,
/// and mirrors close requests. It is generic over `Read`/`Write` so a caller
/// can use a TCP stream, a TLS stream, or a test duplex without coupling this
/// crate to a particular runtime.
pub struct MediaWebSocketTransport<S, M = NoMaskKeySource> {
    stream: S,
    adapter: MediaWebSocketSession,
    media: MediaSession,
    config: MediaWebSocketTransportConfig,
    mask_source: M,
    read_buffer: Vec<u8>,
    pending_writes: VecDeque<PendingWrite>,
    pending_write_bytes: usize,
    close_received: bool,
    close_sent: bool,
    read_closed: bool,
    failed: bool,
}

struct PendingWrite {
    bytes: Vec<u8>,
    offset: usize,
}

impl<S: Read + Write> MediaWebSocketTransport<S, NoMaskKeySource> {
    /// Creates a server-role stream driver without a masking-key source.
    ///
    /// # Errors
    ///
    /// Returns an error when the adapter or transport bounds are invalid.
    pub fn new(
        stream: S,
        adapter: MediaWebSocketSession,
        media: MediaSession,
        config: MediaWebSocketTransportConfig,
    ) -> Result<Self, TransportError> {
        Self::with_mask_source(stream, adapter, media, config, NoMaskKeySource)
    }
}

impl<S: Read + Write, M: MaskKeySource> MediaWebSocketTransport<S, M> {
    /// Creates a stream driver with an application-provided masking-key source.
    ///
    /// A server-role adapter ignores the source. A client-role adapter calls it
    /// for every outbound data or control frame, including automatic pong and
    /// close replies.
    ///
    /// # Errors
    ///
    /// Returns an error when the adapter or transport bounds are invalid.
    pub fn with_mask_source(
        stream: S,
        adapter: MediaWebSocketSession,
        media: MediaSession,
        config: MediaWebSocketTransportConfig,
        mask_source: M,
    ) -> Result<Self, TransportError> {
        let config = config.validate(&adapter)?;
        Ok(Self {
            stream,
            adapter,
            media,
            config,
            mask_source,
            read_buffer: Vec::new(),
            pending_writes: VecDeque::new(),
            pending_write_bytes: 0,
            close_received: false,
            close_sent: false,
            read_closed: false,
            failed: false,
        })
    }

    /// Returns the transport resource bounds.
    #[must_use]
    pub const fn config(&self) -> MediaWebSocketTransportConfig {
        self.config
    }

    /// Borrows the underlying stream.
    #[must_use]
    pub const fn stream(&self) -> &S {
        &self.stream
    }

    /// Mutably borrows the underlying stream.
    pub fn stream_mut(&mut self) -> &mut S {
        &mut self.stream
    }

    /// Borrows the protocol/media adapter.
    #[must_use]
    pub const fn adapter(&self) -> &MediaWebSocketSession {
        &self.adapter
    }

    /// Mutably borrows the protocol/media adapter.
    pub fn adapter_mut(&mut self) -> &mut MediaWebSocketSession {
        &mut self.adapter
    }

    /// Borrows the RTP/media session driven by this stream.
    #[must_use]
    pub const fn media(&self) -> &MediaSession {
        &self.media
    }

    /// Mutably borrows the RTP/media session driven by this stream.
    pub fn media_mut(&mut self) -> &mut MediaSession {
        &mut self.media
    }

    /// Returns the number of encoded bytes waiting to be written.
    #[must_use]
    pub const fn pending_write_bytes(&self) -> usize {
        self.pending_write_bytes
    }

    /// Returns the number of frames waiting to be written.
    #[must_use]
    pub fn pending_write_frames(&self) -> usize {
        self.pending_writes.len()
    }

    /// Returns whether a close handshake has started.
    #[must_use]
    pub const fn is_closing(&self) -> bool {
        self.close_received || self.close_sent
    }

    /// Returns whether the peer has requested a close.
    #[must_use]
    pub const fn close_received(&self) -> bool {
        self.close_received
    }

    /// Returns whether a close frame has been queued or written.
    #[must_use]
    pub const fn close_sent(&self) -> bool {
        self.close_sent
    }

    /// Returns whether terminal failure cleanup has been performed.
    #[must_use]
    pub const fn is_failed(&self) -> bool {
        self.failed
    }

    /// Reclaims all bounded state after an unrecoverable read, write, or
    /// downstream AI disconnect.
    ///
    /// Call this after handling [`TransportError::ConnectionClosed`],
    /// [`TransportError::Io`], [`TransportError::WriteZero`], or another
    /// terminal transport error. It is idempotent: subsequent calls release
    /// no additional state. No close frame is attempted because the caller has
    /// declared the underlying stream unusable.
    pub fn cleanup_after_failure(&mut self) -> MediaWebSocketCleanup {
        let cleanup = MediaWebSocketCleanup {
            stream: self.adapter.reset(),
            buffered_read_bytes: self.read_buffer.len(),
            pending_write_frames: self.pending_writes.len(),
            pending_write_bytes: self.pending_write_bytes,
            media: self.media.reclaim_pending(),
        };
        self.read_buffer.clear();
        self.pending_writes.clear();
        self.pending_write_bytes = 0;
        self.read_closed = true;
        self.failed = true;
        cleanup
    }

    /// Reads at most one bounded chunk and processes all complete frames now
    /// available. Incomplete frames remain buffered for the next call.
    ///
    /// Ping frames cause a matching pong to be queued before this method
    /// returns. A close frame is surfaced as an event and a matching close
    /// response is queued once. Call [`Self::flush`] to write queued replies.
    ///
    /// # Errors
    ///
    /// Returns an error for stream I/O, malformed media, an exhausted read
    /// bound, or EOF. EOF reports how many partial bytes were retained.
    pub fn read_once(&mut self) -> Result<Vec<MediaWebSocketEvent>, TransportError> {
        if self.close_received || self.read_closed {
            return Err(TransportError::Closed);
        }

        let mut events = self.process_buffer()?;
        if events.is_empty() && self.read_buffer.len() < self.config.max_buffered_bytes {
            let remaining = self.config.max_buffered_bytes - self.read_buffer.len();
            let amount = remaining.min(self.config.max_read_bytes);
            let mut chunk = vec![0; amount];
            let count = self.stream.read(&mut chunk)?;
            if count == 0 {
                self.read_closed = true;
                return Err(TransportError::ConnectionClosed {
                    buffered_bytes: self.read_buffer.len(),
                });
            }
            self.read_buffer.extend_from_slice(&chunk[..count]);
            events.extend(self.process_buffer()?);
        }
        if events.is_empty() && self.read_buffer.len() == self.config.max_buffered_bytes {
            return Err(TransportError::ReadBufferFull {
                actual: self.read_buffer.len(),
                maximum: self.config.max_buffered_bytes,
            });
        }
        Ok(events)
    }

    /// Queues one application control command for writing.
    ///
    /// The adapter's stream state is advanced by inbound `MEDIA_START` as in
    /// [`MediaWebSocketSession`]; callers should therefore send controls only
    /// after the corresponding protocol state is established.
    pub fn queue_command(&mut self, command: &MediaCommand) -> Result<(), TransportError> {
        self.ensure_writable()?;
        let mask_key = self.next_mask_key()?;
        let wire = self.adapter.encode_control(command, mask_key)?;
        self.queue_wire(wire)
    }

    /// Queues the next AI-bound audio frame, if one is available.
    ///
    /// Queue capacity is checked before asking the adapter to consume a media
    /// frame. A full output queue therefore never drops the queued audio.
    ///
    /// # Returns
    ///
    /// `Ok(true)` when one frame was queued and `Ok(false)` when the media
    /// queue had no frame or the negotiated direction suppresses output.
    pub fn queue_audio(&mut self) -> Result<bool, TransportError> {
        self.ensure_writable()?;
        if self.media.peek_for_ai().is_none() {
            return Ok(false);
        }
        self.ensure_wire_capacity()?;
        let mask_key = self.next_mask_key()?;
        let Some(wire) = self.adapter.next_audio(&mut self.media, mask_key)? else {
            return Ok(false);
        };
        self.queue_wire(wire)?;
        Ok(true)
    }

    /// Queues a validated close frame and starts the close handshake.
    pub fn queue_close(&mut self, close: CloseFrame) -> Result<(), TransportError> {
        if self.close_sent {
            return Ok(());
        }
        if self.read_closed {
            return Err(TransportError::Closed);
        }
        let frame = WebSocketFrame::close(close).map_err(MediaWebSocketError::WebSocket)?;
        let mask_key = self.next_mask_key()?;
        let wire = frame
            .encode(self.adapter.config().websocket, mask_key)
            .map_err(MediaWebSocketError::WebSocket)?;
        self.queue_wire(wire)?;
        self.close_sent = true;
        Ok(())
    }

    /// Writes queued frames, retaining any partially written frame.
    ///
    /// Returns the number of bytes accepted by the underlying stream. A zero
    /// write is reported as [`TransportError::WriteZero`] instead of spinning.
    pub fn flush(&mut self) -> Result<usize, TransportError> {
        if self.read_closed && self.pending_writes.is_empty() {
            return Err(TransportError::Closed);
        }
        let mut written_total = 0;
        while let Some(front) = self.pending_writes.front_mut() {
            let written = self.stream.write(&front.bytes[front.offset..])?;
            if written == 0 {
                return Err(TransportError::WriteZero);
            }
            front.offset += written;
            self.pending_write_bytes -= written;
            written_total += written;
            if front.offset == front.bytes.len() {
                self.pending_writes.pop_front();
            }
        }
        self.stream.flush()?;
        Ok(written_total)
    }

    /// Consumes the driver and returns its stream, adapter, and media session.
    #[must_use]
    pub fn into_parts(self) -> (S, MediaWebSocketSession, MediaSession) {
        (self.stream, self.adapter, self.media)
    }

    fn process_buffer(&mut self) -> Result<Vec<MediaWebSocketEvent>, TransportError> {
        let mut events = Vec::new();
        while !self.read_buffer.is_empty() {
            let decoded = match self.adapter.receive(&self.read_buffer, &mut self.media) {
                Ok(decoded) => decoded,
                Err(MediaWebSocketError::WebSocket(crate::WebSocketError::Incomplete)) => break,
                Err(error) => return Err(error.into()),
            };
            let (frame_events, consumed) = decoded;
            if consumed == 0 || consumed > self.read_buffer.len() {
                return Err(TransportError::InvalidConfig);
            }
            self.read_buffer.drain(..consumed);
            for event in frame_events {
                self.handle_event(&event)?;
                events.push(event);
            }
            if self.close_received {
                break;
            }
        }
        Ok(events)
    }

    fn handle_event(&mut self, event: &MediaWebSocketEvent) -> Result<(), TransportError> {
        match event {
            MediaWebSocketEvent::Ping(payload) => {
                self.queue_frame(WebSocketFrame::new(true, OpCode::Pong, payload.clone()))
            }
            MediaWebSocketEvent::Close(close) => {
                self.close_received = true;
                if !self.close_sent {
                    self.queue_close(close.clone())?;
                }
                Ok(())
            }
            MediaWebSocketEvent::Command(_)
            | MediaWebSocketEvent::Audio { .. }
            | MediaWebSocketEvent::Pong(_) => Ok(()),
        }
    }

    fn ensure_writable(&self) -> Result<(), TransportError> {
        if self.close_received || self.close_sent || self.read_closed {
            Err(TransportError::Closed)
        } else {
            Ok(())
        }
    }

    fn ensure_wire_capacity(&self) -> Result<(), TransportError> {
        if self.pending_writes.len() >= self.config.max_pending_writes {
            return Err(self.write_queue_full(1, 0));
        }
        let maximum_wire = self
            .adapter
            .config()
            .websocket
            .max_frame_bytes
            .checked_add(MAX_FRAME_WIRE_OVERHEAD)
            .ok_or(TransportError::InvalidConfig)?;
        if self.pending_write_bytes > self.config.max_pending_write_bytes - maximum_wire {
            return Err(self.write_queue_full(1, maximum_wire));
        }
        Ok(())
    }

    fn queue_frame(&mut self, frame: WebSocketFrame) -> Result<(), TransportError> {
        let mask_key = self.next_mask_key()?;
        let wire = frame
            .encode(self.adapter.config().websocket, mask_key)
            .map_err(MediaWebSocketError::WebSocket)?;
        self.queue_wire(wire)
    }

    fn queue_wire(&mut self, wire: Vec<u8>) -> Result<(), TransportError> {
        if self.pending_writes.len() >= self.config.max_pending_writes
            || self.pending_write_bytes
                > self
                    .config
                    .max_pending_write_bytes
                    .saturating_sub(wire.len())
        {
            return Err(self.write_queue_full(1, wire.len()));
        }
        self.pending_write_bytes += wire.len();
        self.pending_writes.push_back(PendingWrite {
            bytes: wire,
            offset: 0,
        });
        Ok(())
    }

    fn write_queue_full(&self, frames: usize, bytes: usize) -> TransportError {
        TransportError::WriteQueueFull {
            frames: self.pending_writes.len().saturating_add(frames),
            bytes: self.pending_write_bytes.saturating_add(bytes),
            maximum_frames: self.config.max_pending_writes,
            maximum_bytes: self.config.max_pending_write_bytes,
        }
    }

    fn next_mask_key(&mut self) -> Result<Option<[u8; 4]>, TransportError> {
        if self.adapter.config().websocket.role == WebSocketRole::Client {
            Ok(Some(
                self.mask_source
                    .next_mask_key()
                    .map_err(TransportError::MaskKey)?,
            ))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        io::{Cursor, Read, Result as IoResult, Write},
    };

    use media_core::{AudioFrame, MediaBridgeConfig, MediaReclamation, MediaSessionConfig};
    use rtp::{RtpSession, RtpSessionConfig};

    use super::*;
    use crate::{MediaDirection, MediaWebSocketConfig, OpCode, WebSocketConfig};

    #[derive(Debug)]
    struct TestStream {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
        max_read: usize,
        max_write: usize,
    }

    impl TestStream {
        fn new(input: Vec<u8>, max_read: usize, max_write: usize) -> Self {
            Self {
                input: Cursor::new(input),
                output: Vec::new(),
                max_read,
                max_write,
            }
        }
    }

    impl Read for TestStream {
        fn read(&mut self, target: &mut [u8]) -> IoResult<usize> {
            let amount = target.len().min(self.max_read);
            self.input.read(&mut target[..amount])
        }
    }

    impl Write for TestStream {
        fn write(&mut self, source: &[u8]) -> IoResult<usize> {
            let amount = source.len().min(self.max_write);
            self.output.extend_from_slice(&source[..amount]);
            Ok(amount)
        }

        fn flush(&mut self) -> IoResult<()> {
            Ok(())
        }
    }

    #[derive(Debug)]
    struct FixedKeys {
        keys: VecDeque<[u8; 4]>,
    }

    impl MaskKeySource for FixedKeys {
        fn next_mask_key(&mut self) -> Result<[u8; 4], MaskKeyError> {
            self.keys.pop_front().ok_or(MaskKeyError::Unavailable)
        }
    }

    fn websocket_config(role: WebSocketRole) -> WebSocketConfig {
        WebSocketConfig {
            role,
            max_frame_bytes: 256,
            max_message_bytes: 1_024,
            max_fragments: 4,
        }
    }

    fn adapter(role: WebSocketRole) -> MediaWebSocketSession {
        MediaWebSocketSession::new(MediaWebSocketConfig {
            websocket: websocket_config(role),
            max_frame_samples: 4,
            ..MediaWebSocketConfig::default()
        })
        .unwrap()
    }

    fn media() -> MediaSession {
        MediaSession::new(
            MediaSessionConfig {
                rtp: RtpSessionConfig {
                    payload_type: 0,
                    ..RtpSessionConfig::default()
                },
                bridge: MediaBridgeConfig {
                    to_ai_capacity: 2,
                    from_ai_capacity: 2,
                    ..MediaBridgeConfig::default()
                },
                ..MediaSessionConfig::default()
            },
            1,
            1,
        )
        .unwrap()
    }

    fn transport_config() -> MediaWebSocketTransportConfig {
        MediaWebSocketTransportConfig {
            max_read_bytes: 1_024,
            max_buffered_bytes: 2_048,
            max_pending_writes: 4,
            max_pending_write_bytes: 2_048,
        }
    }

    fn wire(frame: WebSocketFrame, sender_role: WebSocketRole) -> Vec<u8> {
        frame
            .encode(
                websocket_config(sender_role),
                (sender_role == WebSocketRole::Client).then_some([1, 2, 3, 4]),
            )
            .unwrap()
    }

    fn start_frame(sender_role: WebSocketRole) -> Vec<u8> {
        wire(
            WebSocketFrame::text(
                "MEDIA_START connection_id:INCOMING channel_id:chan-1 format:ulaw optimal_frame_size:4",
            ),
            sender_role,
        )
    }

    fn receive_start<S: Read + Write, M: MaskKeySource>(
        transport: &mut MediaWebSocketTransport<S, M>,
    ) {
        let events = transport.read_once().unwrap();
        assert!(events.iter().any(|event| {
            matches!(
                event,
                MediaWebSocketEvent::Command(MediaCommand::Start(start))
                    if start.direction == MediaDirection::Both
            )
        }));
    }

    #[test]
    fn reads_frames_and_queues_automatic_pong() {
        let mut input = start_frame(WebSocketRole::Client);
        input.extend(wire(
            WebSocketFrame::binary(vec![0xff, 0xce, 0x4e, 0x00]),
            WebSocketRole::Client,
        ));
        input.extend(wire(
            WebSocketFrame::new(true, OpCode::Ping, vec![9, 8]),
            WebSocketRole::Client,
        ));
        let stream = TestStream::new(input, 2_048, 2_048);
        let mut transport = MediaWebSocketTransport::new(
            stream,
            adapter(WebSocketRole::Server),
            media(),
            transport_config(),
        )
        .unwrap();

        let events = transport.read_once().unwrap();
        assert_eq!(events.len(), 3);
        assert!(matches!(events[1], MediaWebSocketEvent::Audio { .. }));
        assert!(matches!(events[2], MediaWebSocketEvent::Ping(ref payload) if payload == &[9, 8]));
        assert_eq!(transport.media().stats().bridge.from_ai.depth, 1);
        assert_eq!(transport.pending_write_frames(), 1);
        transport.flush().unwrap();

        let output = &transport.stream().output;
        let (frame, consumed) =
            WebSocketFrame::decode(output, websocket_config(WebSocketRole::Client)).unwrap();
        assert_eq!(consumed, output.len());
        assert_eq!(frame.opcode, OpCode::Pong);
        assert_eq!(frame.payload, vec![9, 8]);
    }

    #[test]
    fn retains_partial_frames_between_reads() {
        let stream = TestStream::new(start_frame(WebSocketRole::Client), 2, 2_048);
        let mut transport = MediaWebSocketTransport::new(
            stream,
            adapter(WebSocketRole::Server),
            media(),
            MediaWebSocketTransportConfig {
                max_read_bytes: 2,
                ..transport_config()
            },
        )
        .unwrap();
        let mut saw_start = false;
        for _ in 0..100 {
            let events = transport.read_once().unwrap();
            if events
                .iter()
                .any(|event| matches!(event, MediaWebSocketEvent::Command(MediaCommand::Start(_))))
            {
                saw_start = true;
                break;
            }
        }
        assert!(saw_start);
        assert!(transport.adapter().stream().is_some());
    }

    #[test]
    fn output_queue_backpressure_preserves_audio() {
        let stream = TestStream::new(start_frame(WebSocketRole::Client), 2_048, 2_048);
        let mut transport = MediaWebSocketTransport::new(
            stream,
            adapter(WebSocketRole::Server),
            media(),
            MediaWebSocketTransportConfig {
                max_pending_writes: 1,
                ..transport_config()
            },
        )
        .unwrap();
        receive_start(&mut transport);
        transport.queue_command(&MediaCommand::Answer).unwrap();

        let mut sender = RtpSession::new(
            RtpSessionConfig {
                payload_type: 0,
                ..RtpSessionConfig::default()
            },
            1,
            1,
        )
        .unwrap();
        let packet = sender.send(&[0xff; 4], 4, false).unwrap();
        transport
            .media_mut()
            .receive_rtp(&packet, std::time::Duration::ZERO)
            .unwrap();

        assert!(matches!(
            transport.queue_audio(),
            Err(TransportError::WriteQueueFull { .. })
        ));
        assert_eq!(transport.media().stats().bridge.to_ai.depth, 1);
    }

    #[test]
    fn flush_handles_partial_writes_and_tracks_pending_bytes() {
        let stream = TestStream::new(Vec::new(), 2_048, 2);
        let mut transport = MediaWebSocketTransport::new(
            stream,
            adapter(WebSocketRole::Server),
            media(),
            transport_config(),
        )
        .unwrap();
        transport.queue_command(&MediaCommand::Answer).unwrap();
        let pending = transport.pending_write_bytes();
        assert!(pending > 2);
        assert_eq!(transport.flush().unwrap(), pending);
        assert_eq!(transport.pending_write_bytes(), 0);
        assert_eq!(transport.pending_write_frames(), 0);
    }

    #[test]
    fn flush_reports_zero_writes_without_dropping_the_frame() {
        let stream = TestStream::new(Vec::new(), 2_048, 0);
        let mut transport = MediaWebSocketTransport::new(
            stream,
            adapter(WebSocketRole::Server),
            media(),
            transport_config(),
        )
        .unwrap();
        transport.queue_command(&MediaCommand::Answer).unwrap();
        assert!(matches!(transport.flush(), Err(TransportError::WriteZero)));
        assert_eq!(transport.pending_write_frames(), 1);
        assert!(transport.pending_write_bytes() > 0);
    }

    #[test]
    fn client_role_uses_fresh_mask_source_for_writes() {
        let stream = TestStream::new(
            wire(
                WebSocketFrame::text(
                    "MEDIA_START connection_id:x channel_id:y format:ulaw optimal_frame_size:4",
                ),
                WebSocketRole::Server,
            ),
            2_048,
            2_048,
        );
        let keys = FixedKeys {
            keys: VecDeque::from([[4, 3, 2, 1]]),
        };
        let mut transport = MediaWebSocketTransport::with_mask_source(
            stream,
            adapter(WebSocketRole::Client),
            media(),
            transport_config(),
            keys,
        )
        .unwrap();
        receive_start(&mut transport);
        transport.queue_command(&MediaCommand::Answer).unwrap();
        transport.flush().unwrap();

        let output = &transport.stream().output;
        assert_ne!(output[1] & 0x80, 0);
        let (frame, consumed) =
            WebSocketFrame::decode(output, websocket_config(WebSocketRole::Server)).unwrap();
        assert_eq!(consumed, output.len());
        assert_eq!(frame.opcode, OpCode::Text);
        assert_eq!(frame.payload, b"ANSWER");
    }

    #[test]
    fn close_is_mirrored_once_and_then_stream_is_closing() {
        let close = WebSocketFrame::close(CloseFrame {
            code: 1_000,
            reason: "done".to_owned(),
        })
        .unwrap();
        let stream = TestStream::new(wire(close, WebSocketRole::Client), 2_048, 2_048);
        let mut transport = MediaWebSocketTransport::new(
            stream,
            adapter(WebSocketRole::Server),
            media(),
            transport_config(),
        )
        .unwrap();
        let events = transport.read_once().unwrap();
        assert!(
            matches!(events.as_slice(), [MediaWebSocketEvent::Close(close)] if close.code == 1_000)
        );
        assert!(transport.is_closing());
        assert!(transport.close_received());
        assert!(transport.close_sent());
        transport.flush().unwrap();
        let (frame, _) = WebSocketFrame::decode(
            &transport.stream().output,
            websocket_config(WebSocketRole::Client),
        )
        .unwrap();
        assert_eq!(frame.opcode, OpCode::Close);
    }

    #[test]
    fn eof_reports_retained_partial_bytes() {
        let stream = TestStream::new(vec![0x82], 2_048, 2_048);
        let mut transport = MediaWebSocketTransport::new(
            stream,
            adapter(WebSocketRole::Server),
            media(),
            transport_config(),
        )
        .unwrap();
        assert!(transport.read_once().unwrap().is_empty());
        assert!(matches!(
            transport.read_once(),
            Err(TransportError::ConnectionClosed { buffered_bytes: 1 })
        ));
    }

    #[test]
    fn failure_cleanup_reclaims_partial_io_pending_writes_and_media_once() {
        let mut input = start_frame(WebSocketRole::Client);
        input.push(0x82);
        let stream = TestStream::new(input, 2_048, 2_048);
        let mut transport = MediaWebSocketTransport::new(
            stream,
            adapter(WebSocketRole::Server),
            media(),
            transport_config(),
        )
        .unwrap();
        receive_start(&mut transport);
        transport.media_mut().push_from_ai(AudioFrame {
            timestamp: 1,
            codec: media_core::AudioCodec::Pcmu,
            sample_rate: 8_000,
            samples: vec![1, 2, 3, 4],
        });
        transport.queue_command(&MediaCommand::Answer).unwrap();
        assert!(matches!(
            transport.read_once(),
            Err(TransportError::ConnectionClosed { buffered_bytes: 1 })
        ));

        let cleanup = transport.cleanup_after_failure();
        assert_eq!(cleanup.stream.unwrap().connection_id, "INCOMING");
        assert_eq!(cleanup.buffered_read_bytes, 1);
        assert_eq!(cleanup.pending_write_frames, 1);
        assert!(cleanup.pending_write_bytes > 0);
        assert_eq!(cleanup.media.from_ai_frames, 1);
        assert_eq!(cleanup.media.to_ai_frames, 0);
        assert_eq!(cleanup.media.jitter_packets, 0);
        assert_eq!(cleanup.media.dtmf_notifications, 0);
        assert!(transport.is_failed());
        assert_eq!(transport.pending_write_frames(), 0);
        assert_eq!(transport.pending_write_bytes(), 0);
        assert!(transport.adapter().stream().is_none());
        assert_eq!(transport.media().stats().bridge.from_ai.depth, 0);
        assert_eq!(
            transport.cleanup_after_failure(),
            MediaWebSocketCleanup {
                stream: None,
                buffered_read_bytes: 0,
                pending_write_frames: 0,
                pending_write_bytes: 0,
                media: MediaReclamation::default(),
            }
        );
        assert!(matches!(transport.read_once(), Err(TransportError::Closed)));
    }
}
