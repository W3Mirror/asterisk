//! Bounded WebSocket framing and G.711 media bridging for AI applications.
//!
//! An HTTP/WebSocket listener owns TLS and the upgrade handshake, then can pass
//! the upgraded stream to [`MediaWebSocketTransport`]. The transport owns
//! bounded stream buffering and writes while [`MediaWebSocketSession`]
//! enforces RFC 6455 framing bounds, reconstructs fragmented messages, and
//! translates the plain-text control messages and binary G.711 frames used by
//! Asterisk's `chan_websocket` into [`media_core::MediaSession`] operations.

use std::{
    error::Error,
    fmt::{Display, Formatter},
    str,
};

use media_core::{AudioCodec, AudioFrame, MediaSession, PushOutcome};

mod transport;

pub use transport::{
    MaskKeyError, MaskKeySource, MediaWebSocketCleanup, MediaWebSocketTransport,
    MediaWebSocketTransportConfig, NoMaskKeySource, OsRandomMaskKeySource, TransportError,
};

const DEFAULT_MAX_FRAME_BYTES: usize = 64 * 1024;
const DEFAULT_MAX_MESSAGE_BYTES: usize = 256 * 1024;
const DEFAULT_MAX_FRAGMENTS: usize = 32;
const DEFAULT_MAX_CONTROL_BYTES: usize = 4 * 1024;
const DEFAULT_MAX_IDENTIFIER_BYTES: usize = 256;
const DEFAULT_MAX_FRAME_SAMPLES: usize = 16_000;
const DEFAULT_SAMPLE_RATE: u32 = 8_000;

/// Whether a WebSocket endpoint is acting as a server or client.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WebSocketRole {
    /// Incoming frames must be masked and outgoing frames are unmasked.
    Server,
    /// Incoming frames must be unmasked and outgoing frames are masked.
    Client,
}

/// RFC 6455 frame opcode supported by the bounded codec.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpCode {
    /// A continuation of a fragmented text or binary message.
    Continuation,
    /// A complete or fragmented UTF-8 text message.
    Text,
    /// A complete or fragmented binary message.
    Binary,
    /// A close control frame.
    Close,
    /// A ping control frame.
    Ping,
    /// A pong control frame.
    Pong,
}

impl OpCode {
    fn from_wire(value: u8) -> Result<Self, WebSocketError> {
        match value {
            0x0 => Ok(Self::Continuation),
            0x1 => Ok(Self::Text),
            0x2 => Ok(Self::Binary),
            0x8 => Ok(Self::Close),
            0x9 => Ok(Self::Ping),
            0xa => Ok(Self::Pong),
            other => Err(WebSocketError::InvalidOpcode(other)),
        }
    }

    const fn is_control(self) -> bool {
        matches!(self, Self::Close | Self::Ping | Self::Pong)
    }
}

/// Bounds and masking role for one WebSocket endpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WebSocketConfig {
    /// Endpoint role used to enforce the RFC masking direction.
    pub role: WebSocketRole,
    /// Maximum payload bytes in one WebSocket frame.
    pub max_frame_bytes: usize,
    /// Maximum bytes in one reconstructed text or binary message.
    pub max_message_bytes: usize,
    /// Maximum data frames in one fragmented message.
    pub max_fragments: usize,
}

impl Default for WebSocketConfig {
    fn default() -> Self {
        Self {
            role: WebSocketRole::Server,
            max_frame_bytes: DEFAULT_MAX_FRAME_BYTES,
            max_message_bytes: DEFAULT_MAX_MESSAGE_BYTES,
            max_fragments: DEFAULT_MAX_FRAGMENTS,
        }
    }
}

impl WebSocketConfig {
    fn validate(self) -> Result<Self, WebSocketError> {
        if self.max_frame_bytes == 0
            || self.max_message_bytes < self.max_frame_bytes
            || self.max_fragments == 0
        {
            return Err(WebSocketError::InvalidConfig);
        }
        Ok(self)
    }
}

/// Errors raised by WebSocket framing or message reconstruction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WebSocketError {
    /// A configured bound was zero or inconsistent.
    InvalidConfig,
    /// More bytes are needed before a complete frame can be decoded.
    Incomplete,
    /// RSV bits were set without a negotiated extension.
    ReservedBits,
    /// The opcode is not supported by this codec.
    InvalidOpcode(u8),
    /// A server received an unmasked client frame.
    MaskRequired,
    /// A client received a masked server frame.
    UnexpectedMask,
    /// A client frame was configured without a mask key for encoding.
    MaskKeyRequired,
    /// A server frame was given a mask key for encoding.
    MaskKeyNotAllowed,
    /// The frame payload exceeded its configured bound.
    FrameTooLarge { actual: usize, maximum: usize },
    /// A control frame was fragmented.
    FragmentedControl,
    /// A control frame exceeded the RFC 6455 125-byte limit.
    ControlFrameTooLarge,
    /// A continuation frame appeared without an open fragmented message.
    UnexpectedContinuation,
    /// A new data message appeared before the previous one finished.
    UnexpectedDataFrame,
    /// A fragmented message exceeded its configured bounds.
    MessageTooLarge { actual: usize, maximum: usize },
    /// A fragmented message exceeded its frame-count bound.
    TooManyFragments,
    /// A text message was not valid UTF-8.
    InvalidText,
    /// A close payload was malformed or used a reserved status code.
    InvalidClose,
}

impl Display for WebSocketError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfig => formatter.write_str("WebSocket bounds are invalid"),
            Self::Incomplete => formatter.write_str("WebSocket frame is incomplete"),
            Self::ReservedBits => formatter.write_str("WebSocket RSV bits are not negotiated"),
            Self::InvalidOpcode(opcode) => {
                write!(formatter, "unsupported WebSocket opcode {opcode:#x}")
            }
            Self::MaskRequired => formatter.write_str("server requires masked client frames"),
            Self::UnexpectedMask => formatter.write_str("client received a masked server frame"),
            Self::MaskKeyRequired => {
                formatter.write_str("client frame encoding requires a mask key")
            }
            Self::MaskKeyNotAllowed => {
                formatter.write_str("server frame encoding cannot use a mask key")
            }
            Self::FrameTooLarge { actual, maximum } => {
                write!(
                    formatter,
                    "WebSocket frame is {actual} bytes, maximum is {maximum}"
                )
            }
            Self::FragmentedControl => {
                formatter.write_str("WebSocket control frames must not be fragmented")
            }
            Self::ControlFrameTooLarge => {
                formatter.write_str("WebSocket control frame exceeds 125 bytes")
            }
            Self::UnexpectedContinuation => {
                formatter.write_str("unexpected WebSocket continuation frame")
            }
            Self::UnexpectedDataFrame => {
                formatter.write_str("WebSocket data frame interrupted fragmentation")
            }
            Self::MessageTooLarge { actual, maximum } => {
                write!(
                    formatter,
                    "WebSocket message is {actual} bytes, maximum is {maximum}"
                )
            }
            Self::TooManyFragments => {
                formatter.write_str("WebSocket message has too many fragments")
            }
            Self::InvalidText => formatter.write_str("WebSocket text message is not UTF-8"),
            Self::InvalidClose => formatter.write_str("WebSocket close payload is invalid"),
        }
    }
}

impl Error for WebSocketError {}

/// One decoded WebSocket frame without a mask key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebSocketFrame {
    /// Whether this frame completes its message.
    pub fin: bool,
    /// Frame opcode.
    pub opcode: OpCode,
    /// Unmasked payload bytes.
    pub payload: Vec<u8>,
}

impl WebSocketFrame {
    /// Creates a frame from an already unmasked payload.
    #[must_use]
    pub const fn new(fin: bool, opcode: OpCode, payload: Vec<u8>) -> Self {
        Self {
            fin,
            opcode,
            payload,
        }
    }

    /// Creates a complete binary data frame.
    #[must_use]
    pub fn binary(payload: Vec<u8>) -> Self {
        Self::new(true, OpCode::Binary, payload)
    }

    /// Creates a complete UTF-8 text data frame.
    #[must_use]
    pub fn text(payload: impl Into<String>) -> Self {
        Self::new(true, OpCode::Text, payload.into().into_bytes())
    }

    /// Creates a ping control frame.
    #[must_use]
    pub fn ping(payload: Vec<u8>) -> Self {
        Self::new(true, OpCode::Ping, payload)
    }

    /// Creates a pong control frame.
    #[must_use]
    pub fn pong(payload: Vec<u8>) -> Self {
        Self::new(true, OpCode::Pong, payload)
    }

    /// Creates a close frame with a status code and UTF-8 reason.
    pub fn close(close: CloseFrame) -> Result<Self, WebSocketError> {
        validate_close_code(close.code)?;
        if close.reason.len() > 123 {
            return Err(WebSocketError::ControlFrameTooLarge);
        }
        let mut payload = Vec::with_capacity(2 + close.reason.len());
        payload.extend_from_slice(&close.code.to_be_bytes());
        payload.extend_from_slice(close.reason.as_bytes());
        Ok(Self::new(true, OpCode::Close, payload))
    }

    /// Decodes one frame and returns the number of consumed input bytes.
    ///
    /// The caller may retain unconsumed bytes and call this method again when
    /// a stream read produced multiple frames or a partial trailing frame.
    pub fn decode(input: &[u8], config: WebSocketConfig) -> Result<(Self, usize), WebSocketError> {
        let config = config.validate()?;
        if input.len() < 2 {
            return Err(WebSocketError::Incomplete);
        }
        let first = input[0];
        if first & 0x70 != 0 {
            return Err(WebSocketError::ReservedBits);
        }
        let fin = first & 0x80 != 0;
        let opcode = OpCode::from_wire(first & 0x0f)?;
        let second = input[1];
        let masked = second & 0x80 != 0;
        match (config.role, masked) {
            (WebSocketRole::Server, false) => return Err(WebSocketError::MaskRequired),
            (WebSocketRole::Client, true) => return Err(WebSocketError::UnexpectedMask),
            _ => {}
        }
        let length_marker = second & 0x7f;
        let (payload_len, mut offset) = match length_marker {
            0..=125 => (usize::from(length_marker), 2usize),
            126 => {
                if input.len() < 4 {
                    return Err(WebSocketError::Incomplete);
                }
                (
                    usize::from(u16::from_be_bytes([input[2], input[3]])),
                    4usize,
                )
            }
            127 => {
                if input.len() < 10 {
                    return Err(WebSocketError::Incomplete);
                }
                let length = u64::from_be_bytes([
                    input[2], input[3], input[4], input[5], input[6], input[7], input[8], input[9],
                ]);
                if length & (1 << 63) != 0 {
                    return Err(WebSocketError::FrameTooLarge {
                        actual: usize::MAX,
                        maximum: config.max_frame_bytes,
                    });
                }
                (
                    usize::try_from(length).map_err(|_| WebSocketError::FrameTooLarge {
                        actual: usize::MAX,
                        maximum: config.max_frame_bytes,
                    })?,
                    10usize,
                )
            }
            _ => unreachable!(),
        };
        if payload_len > config.max_frame_bytes {
            return Err(WebSocketError::FrameTooLarge {
                actual: payload_len,
                maximum: config.max_frame_bytes,
            });
        }
        if opcode.is_control() {
            if !fin {
                return Err(WebSocketError::FragmentedControl);
            }
            if payload_len > 125 {
                return Err(WebSocketError::ControlFrameTooLarge);
            }
        }
        let mask = if masked {
            let key = input
                .get(offset..offset + 4)
                .ok_or(WebSocketError::Incomplete)?;
            offset += 4;
            Some([key[0], key[1], key[2], key[3]])
        } else {
            None
        };
        let end = offset
            .checked_add(payload_len)
            .ok_or(WebSocketError::FrameTooLarge {
                actual: usize::MAX,
                maximum: config.max_frame_bytes,
            })?;
        if input.len() < end {
            return Err(WebSocketError::Incomplete);
        }
        let mut payload = input[offset..end].to_vec();
        if let Some(mask) = mask {
            for (index, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[index % 4];
            }
        }
        if opcode == OpCode::Close {
            parse_close(&payload)?;
        }
        Ok((
            Self {
                fin,
                opcode,
                payload,
            },
            end,
        ))
    }

    /// Encodes one frame using the configured endpoint's masking direction.
    ///
    /// Client frames require a caller-provided mask key. Supplying the key at
    /// this boundary keeps randomness and socket concerns in the outer runtime
    /// while retaining deterministic unit tests here.
    pub fn encode(
        &self,
        config: WebSocketConfig,
        mask_key: Option<[u8; 4]>,
    ) -> Result<Vec<u8>, WebSocketError> {
        let config = config.validate()?;
        if self.payload.len() > config.max_frame_bytes {
            return Err(WebSocketError::FrameTooLarge {
                actual: self.payload.len(),
                maximum: config.max_frame_bytes,
            });
        }
        if self.opcode.is_control() {
            if !self.fin {
                return Err(WebSocketError::FragmentedControl);
            }
            if self.payload.len() > 125 {
                return Err(WebSocketError::ControlFrameTooLarge);
            }
            if self.opcode == OpCode::Close {
                parse_close(&self.payload)?;
            }
        }
        if self.opcode == OpCode::Text && str::from_utf8(&self.payload).is_err() {
            return Err(WebSocketError::InvalidText);
        }
        let client_masks = config.role == WebSocketRole::Client;
        if client_masks && mask_key.is_none() {
            return Err(WebSocketError::MaskKeyRequired);
        }
        if !client_masks && mask_key.is_some() {
            return Err(WebSocketError::MaskKeyNotAllowed);
        }
        let payload_len = self.payload.len();
        let mut output = Vec::with_capacity(payload_len + 14);
        output.push((if self.fin { 0x80 } else { 0 }) | opcode_wire(self.opcode));
        let mask_bit = if client_masks { 0x80 } else { 0 };
        match payload_len {
            0..=125 => output.push(mask_bit | payload_len as u8),
            126..=65_535 => {
                output.push(mask_bit | 126);
                output.extend_from_slice(&(payload_len as u16).to_be_bytes());
            }
            _ => {
                output.push(mask_bit | 127);
                output.extend_from_slice(&(payload_len as u64).to_be_bytes());
            }
        }
        if let Some(mask) = mask_key {
            output.extend_from_slice(&mask);
            output.extend(
                self.payload
                    .iter()
                    .enumerate()
                    .map(|(index, byte)| byte ^ mask[index % 4]),
            );
        } else {
            output.extend_from_slice(&self.payload);
        }
        Ok(output)
    }
}

fn opcode_wire(opcode: OpCode) -> u8 {
    match opcode {
        OpCode::Continuation => 0x0,
        OpCode::Text => 0x1,
        OpCode::Binary => 0x2,
        OpCode::Close => 0x8,
        OpCode::Ping => 0x9,
        OpCode::Pong => 0xa,
    }
}

/// A decoded WebSocket control or reconstructed data message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WebSocketEvent {
    /// A complete UTF-8 text message.
    Text(String),
    /// A complete binary message.
    Binary(Vec<u8>),
    /// A ping that the outer runtime may answer with a pong.
    Ping(Vec<u8>),
    /// A pong control message.
    Pong(Vec<u8>),
    /// A close request.
    Close(CloseFrame),
}

/// Close status and UTF-8 reason from a WebSocket close frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CloseFrame {
    /// RFC 6455 close status code.
    pub code: u16,
    /// Optional human-readable UTF-8 reason.
    pub reason: String,
}

/// Stateful bounded reconstruction of fragmented WebSocket messages.
#[derive(Clone, Debug)]
pub struct WebSocketSession {
    config: WebSocketConfig,
    fragment: Option<FragmentBuffer>,
}

#[derive(Clone, Debug)]
struct FragmentBuffer {
    opcode: OpCode,
    payload: Vec<u8>,
    fragments: usize,
}

impl WebSocketSession {
    /// Creates a message reconstruction session.
    pub fn new(config: WebSocketConfig) -> Result<Self, WebSocketError> {
        Ok(Self {
            config: config.validate()?,
            fragment: None,
        })
    }

    /// Returns the validated session configuration.
    #[must_use]
    pub const fn config(&self) -> WebSocketConfig {
        self.config
    }

    pub(crate) fn reset(&mut self) {
        self.fragment = None;
    }

    /// Accepts one decoded frame and emits zero or one complete event.
    pub fn receive_frame(
        &mut self,
        frame: WebSocketFrame,
    ) -> Result<Option<WebSocketEvent>, WebSocketError> {
        if frame.payload.len() > self.config.max_frame_bytes {
            return Err(WebSocketError::FrameTooLarge {
                actual: frame.payload.len(),
                maximum: self.config.max_frame_bytes,
            });
        }
        if frame.opcode.is_control() {
            if !frame.fin {
                return Err(WebSocketError::FragmentedControl);
            }
            if frame.payload.len() > 125 {
                return Err(WebSocketError::ControlFrameTooLarge);
            }
            return match frame.opcode {
                OpCode::Ping => Ok(Some(WebSocketEvent::Ping(frame.payload))),
                OpCode::Pong => Ok(Some(WebSocketEvent::Pong(frame.payload))),
                OpCode::Close => Ok(Some(WebSocketEvent::Close(parse_close(&frame.payload)?))),
                OpCode::Continuation | OpCode::Text | OpCode::Binary => unreachable!(),
            };
        }
        match frame.opcode {
            OpCode::Text | OpCode::Binary => {
                if self.fragment.is_some() {
                    return Err(WebSocketError::UnexpectedDataFrame);
                }
                if frame.fin {
                    self.complete_message(frame.opcode, frame.payload)
                } else {
                    self.ensure_message_size(frame.payload.len())?;
                    self.fragment = Some(FragmentBuffer {
                        opcode: frame.opcode,
                        payload: frame.payload,
                        fragments: 1,
                    });
                    Ok(None)
                }
            }
            OpCode::Continuation => {
                let Some(mut fragment) = self.fragment.take() else {
                    return Err(WebSocketError::UnexpectedContinuation);
                };
                fragment.fragments = fragment
                    .fragments
                    .checked_add(1)
                    .ok_or(WebSocketError::TooManyFragments)?;
                if fragment.fragments > self.config.max_fragments {
                    return Err(WebSocketError::TooManyFragments);
                }
                self.ensure_message_size(
                    fragment
                        .payload
                        .len()
                        .checked_add(frame.payload.len())
                        .ok_or(WebSocketError::MessageTooLarge {
                            actual: usize::MAX,
                            maximum: self.config.max_message_bytes,
                        })?,
                )?;
                fragment.payload.extend_from_slice(&frame.payload);
                if frame.fin {
                    self.complete_message(fragment.opcode, fragment.payload)
                } else {
                    self.fragment = Some(fragment);
                    Ok(None)
                }
            }
            OpCode::Close | OpCode::Ping | OpCode::Pong => unreachable!(),
        }
    }

    fn ensure_message_size(&self, actual: usize) -> Result<(), WebSocketError> {
        if actual > self.config.max_message_bytes {
            return Err(WebSocketError::MessageTooLarge {
                actual,
                maximum: self.config.max_message_bytes,
            });
        }
        Ok(())
    }

    fn complete_message(
        &self,
        opcode: OpCode,
        payload: Vec<u8>,
    ) -> Result<Option<WebSocketEvent>, WebSocketError> {
        self.ensure_message_size(payload.len())?;
        match opcode {
            OpCode::Text => String::from_utf8(payload)
                .map(WebSocketEvent::Text)
                .map(Some)
                .map_err(|_| WebSocketError::InvalidText),
            OpCode::Binary => Ok(Some(WebSocketEvent::Binary(payload))),
            OpCode::Continuation | OpCode::Close | OpCode::Ping | OpCode::Pong => unreachable!(),
        }
    }
}

fn parse_close(payload: &[u8]) -> Result<CloseFrame, WebSocketError> {
    if payload.is_empty() {
        return Ok(CloseFrame {
            code: 1000,
            reason: String::new(),
        });
    }
    if payload.len() == 1 {
        return Err(WebSocketError::InvalidClose);
    }
    let code = u16::from_be_bytes([payload[0], payload[1]]);
    validate_close_code(code)?;
    let reason = str::from_utf8(&payload[2..]).map_err(|_| WebSocketError::InvalidClose)?;
    Ok(CloseFrame {
        code,
        reason: reason.to_owned(),
    })
}

fn validate_close_code(code: u16) -> Result<(), WebSocketError> {
    match code {
        1000..=1003 | 1007..=1014 | 3000..=4999 => Ok(()),
        _ => Err(WebSocketError::InvalidClose),
    }
}

/// Direction of media from the application's perspective, matching
/// Asterisk's `chan_websocket` option values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaDirection {
    /// Audio may flow in both directions.
    Both,
    /// The application receives audio but does not send media to Asterisk.
    In,
    /// The application sends audio but does not receive media from Asterisk.
    Out,
}

impl MediaDirection {
    fn parse(value: &str) -> Result<Self, MediaWebSocketError> {
        match value.to_ascii_lowercase().as_str() {
            "both" => Ok(Self::Both),
            "in" => Ok(Self::In),
            "out" => Ok(Self::Out),
            _ => Err(MediaWebSocketError::InvalidControl),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Both => "both",
            Self::In => "in",
            Self::Out => "out",
        }
    }
}

/// Metadata announced by a `MEDIA_START` control message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaStart {
    /// Asterisk/WebSocket connection identifier, used as the stream ID.
    pub connection_id: String,
    /// Asterisk channel identifier, used to correlate the call leg.
    pub channel_id: String,
    /// G.711 codec carried by binary media frames.
    pub codec: AudioCodec,
    /// PCM sample rate represented by one encoded byte per sample.
    pub sample_rate: u32,
    /// Number of encoded samples in one normal media frame.
    pub frame_samples: usize,
    /// Current application-facing media direction.
    pub direction: MediaDirection,
}

/// Plain-text controls understood by the bounded media adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MediaCommand {
    /// A media stream was negotiated and is ready for audio.
    Start(MediaStart),
    /// Ask the call leg to answer.
    Answer,
    /// Ask the call leg to hang up.
    Hangup,
    /// Temporarily buffer incoming media.
    StartMediaBuffering,
    /// Stop buffering and flush a correlated media operation.
    StopMediaBuffering { correlation_id: Option<String> },
    /// Pause media reads from the application.
    PauseMedia,
    /// Resume media reads from the application.
    ContinueMedia,
    /// Change the application-facing media direction.
    SetMediaDirection(MediaDirection),
    /// Ask the adapter to report that its inbound queue drained.
    ReportQueueDrained,
}

/// Events produced after decoding one WebSocket media message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MediaWebSocketEvent {
    /// A control command that the call/runtime layer should handle.
    Command(MediaCommand),
    /// One inbound audio frame was decoded and offered to `MediaSession`.
    Audio {
        /// Timestamp assigned by the adapter's bounded media clock.
        timestamp: u32,
        /// Number of decoded PCM samples.
        samples: usize,
        /// Queue outcome from the media session.
        queued: PushOutcome,
    },
    /// A ping control message that should normally receive a matching pong.
    Ping(Vec<u8>),
    /// A pong control message.
    Pong(Vec<u8>),
    /// A close request.
    Close(CloseFrame),
}

/// Errors raised by media controls or G.711 WebSocket bridging.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MediaWebSocketError {
    /// A WebSocket framing error.
    WebSocket(WebSocketError),
    /// A media adapter bound was invalid.
    InvalidConfig,
    /// A text control exceeded its configured bound.
    ControlTooLarge { actual: usize, maximum: usize },
    /// A control message was malformed or unsupported.
    InvalidControl,
    /// A required `MEDIA_START` field was absent.
    MissingField(&'static str),
    /// A codec format was not a supported G.711 value.
    UnsupportedFormat,
    /// A G.711 media frame was empty.
    InvalidMediaFrame,
    /// An audio frame was larger than the configured frame bound.
    AudioFrameTooLarge { actual: usize, maximum: usize },
    /// Binary media arrived before `MEDIA_START`.
    StreamNotStarted,
    /// A media command required an active stream.
    StreamAlreadyStarted,
    /// A frame's codec/rate did not match the active stream.
    MediaFormatMismatch,
}

impl Display for MediaWebSocketError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WebSocket(error) => Display::fmt(error, formatter),
            Self::InvalidConfig => formatter.write_str("media WebSocket bounds are invalid"),
            Self::ControlTooLarge { actual, maximum } => {
                write!(
                    formatter,
                    "media control is {actual} bytes, maximum is {maximum}"
                )
            }
            Self::InvalidControl => formatter.write_str("media WebSocket control is invalid"),
            Self::MissingField(field) => write!(formatter, "MEDIA_START is missing {field}"),
            Self::UnsupportedFormat => {
                formatter.write_str("media WebSocket format is not PCMU or PCMA")
            }
            Self::InvalidMediaFrame => {
                formatter.write_str("media WebSocket binary frame is invalid")
            }
            Self::AudioFrameTooLarge { actual, maximum } => {
                write!(
                    formatter,
                    "audio frame has {actual} samples, maximum is {maximum}"
                )
            }
            Self::StreamNotStarted => formatter.write_str("MEDIA_START has not been received"),
            Self::StreamAlreadyStarted => formatter.write_str("MEDIA_START was already received"),
            Self::MediaFormatMismatch => {
                formatter.write_str("audio frame does not match MEDIA_START")
            }
        }
    }
}

impl Error for MediaWebSocketError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::WebSocket(error) => Some(error),
            _ => None,
        }
    }
}

impl From<WebSocketError> for MediaWebSocketError {
    fn from(error: WebSocketError) -> Self {
        Self::WebSocket(error)
    }
}

/// Bounds for the Asterisk-compatible media adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MediaWebSocketConfig {
    /// RFC 6455 framing and masking configuration.
    pub websocket: WebSocketConfig,
    /// Maximum bytes accepted in one plain-text control message.
    pub max_control_bytes: usize,
    /// Maximum bytes allowed in each connection/channel identifier.
    pub max_identifier_bytes: usize,
    /// Maximum decoded samples represented by one media frame.
    pub max_frame_samples: usize,
}

impl Default for MediaWebSocketConfig {
    fn default() -> Self {
        Self {
            websocket: WebSocketConfig::default(),
            max_control_bytes: DEFAULT_MAX_CONTROL_BYTES,
            max_identifier_bytes: DEFAULT_MAX_IDENTIFIER_BYTES,
            max_frame_samples: DEFAULT_MAX_FRAME_SAMPLES,
        }
    }
}

impl MediaWebSocketConfig {
    fn validate(self) -> Result<Self, MediaWebSocketError> {
        self.websocket
            .validate()
            .map_err(MediaWebSocketError::WebSocket)?;
        if self.max_control_bytes == 0
            || self.max_identifier_bytes == 0
            || self.max_frame_samples == 0
            || self.max_frame_samples > self.websocket.max_frame_bytes
            || self.max_frame_samples > self.websocket.max_message_bytes
        {
            return Err(MediaWebSocketError::InvalidConfig);
        }
        Ok(self)
    }
}

/// Runtime-agnostic adapter for a bidirectional Asterisk/AI media WebSocket.
#[derive(Clone, Debug)]
pub struct MediaWebSocketSession {
    config: MediaWebSocketConfig,
    websocket: WebSocketSession,
    stream: Option<MediaStart>,
    next_timestamp: u32,
    inbound_remainder: Vec<u8>,
}

impl MediaWebSocketSession {
    /// Creates an adapter with a server or client WebSocket role.
    pub fn new(config: MediaWebSocketConfig) -> Result<Self, MediaWebSocketError> {
        let config = config.validate()?;
        Ok(Self {
            websocket: WebSocketSession::new(config.websocket)?,
            config,
            stream: None,
            next_timestamp: 0,
            inbound_remainder: Vec::new(),
        })
    }

    /// Returns the active stream metadata, if `MEDIA_START` was received.
    #[must_use]
    pub fn stream(&self) -> Option<&MediaStart> {
        self.stream.as_ref()
    }

    /// Returns the configured WebSocket endpoint.
    #[must_use]
    pub const fn config(&self) -> MediaWebSocketConfig {
        self.config
    }

    /// Resets negotiated and partially received media state after a terminal
    /// WebSocket disconnect and returns the prior stream metadata.
    ///
    /// Historical framing and media counters remain owned by the associated
    /// [`MediaSession`]. This method only releases the adapter's bounded
    /// fragmented-message, partial-media, timestamp, and negotiated-stream
    /// state so a replacement stream cannot inherit stale protocol data.
    pub fn reset(&mut self) -> Option<MediaStart> {
        let stream = self.stream.take();
        self.websocket.reset();
        self.next_timestamp = 0;
        self.inbound_remainder.clear();
        stream
    }

    /// Decodes one raw WebSocket frame, applies media messages to `media`, and
    /// returns the consumed byte count for stream-buffer management.
    pub fn receive(
        &mut self,
        input: &[u8],
        media: &mut MediaSession,
    ) -> Result<(Vec<MediaWebSocketEvent>, usize), MediaWebSocketError> {
        let (frame, consumed) = WebSocketFrame::decode(input, self.config.websocket)?;
        let events = self.receive_frame(frame, media)?;
        Ok((events, consumed))
    }

    /// Decodes one already-unmasked frame and applies its media payload.
    pub fn receive_frame(
        &mut self,
        frame: WebSocketFrame,
        media: &mut MediaSession,
    ) -> Result<Vec<MediaWebSocketEvent>, MediaWebSocketError> {
        let Some(event) = self.websocket.receive_frame(frame)? else {
            return Ok(Vec::new());
        };
        match event {
            WebSocketEvent::Text(text) => self.receive_control(&text),
            WebSocketEvent::Binary(payload) => self.receive_binary(&payload, media),
            WebSocketEvent::Ping(payload) => Ok(vec![MediaWebSocketEvent::Ping(payload)]),
            WebSocketEvent::Pong(payload) => Ok(vec![MediaWebSocketEvent::Pong(payload)]),
            WebSocketEvent::Close(close) => Ok(vec![MediaWebSocketEvent::Close(close)]),
        }
    }

    /// Encodes one media control as a WebSocket text frame.
    pub fn encode_control(
        &self,
        command: &MediaCommand,
        mask_key: Option<[u8; 4]>,
    ) -> Result<Vec<u8>, MediaWebSocketError> {
        let text = format_control(command, &self.config)?;
        Ok(WebSocketFrame::text(text).encode(self.config.websocket, mask_key)?)
    }

    /// Encodes the next queued AI-bound frame as a raw G.711 binary message.
    ///
    /// The frame remains queued if validation or serialization fails. This is
    /// important for callers that want to correct a codec/rate mismatch rather
    /// than silently lose audio.
    pub fn next_audio(
        &mut self,
        media: &mut MediaSession,
        mask_key: Option<[u8; 4]>,
    ) -> Result<Option<Vec<u8>>, MediaWebSocketError> {
        let Some(stream) = self.stream.as_ref() else {
            return Err(MediaWebSocketError::StreamNotStarted);
        };
        if stream.direction == MediaDirection::Out {
            return Ok(None);
        }
        let Some(frame) = media.peek_for_ai() else {
            return Ok(None);
        };
        if frame.codec != stream.codec || frame.sample_rate != stream.sample_rate {
            return Err(MediaWebSocketError::MediaFormatMismatch);
        }
        if frame.samples.is_empty() || frame.samples.len() > stream.frame_samples {
            return Err(MediaWebSocketError::AudioFrameTooLarge {
                actual: frame.samples.len(),
                maximum: stream.frame_samples,
            });
        }
        let payload = media_core::encode(frame.codec, &frame.samples);
        let wire = WebSocketFrame::binary(payload).encode(self.config.websocket, mask_key)?;
        let _ = media.pop_for_ai();
        Ok(Some(wire))
    }

    fn receive_control(
        &mut self,
        text: &str,
    ) -> Result<Vec<MediaWebSocketEvent>, MediaWebSocketError> {
        if text.len() > self.config.max_control_bytes {
            return Err(MediaWebSocketError::ControlTooLarge {
                actual: text.len(),
                maximum: self.config.max_control_bytes,
            });
        }
        let command = parse_control(text, &self.config)?;
        if let MediaCommand::Start(start) = &command {
            if self.stream.is_some() {
                return Err(MediaWebSocketError::StreamAlreadyStarted);
            }
            self.next_timestamp = 0;
            self.inbound_remainder.clear();
            self.stream = Some(start.clone());
        } else if let MediaCommand::SetMediaDirection(direction) = command {
            let stream = self
                .stream
                .as_mut()
                .ok_or(MediaWebSocketError::StreamNotStarted)?;
            stream.direction = direction;
            return Ok(vec![MediaWebSocketEvent::Command(
                MediaCommand::SetMediaDirection(direction),
            )]);
        }
        Ok(vec![MediaWebSocketEvent::Command(command)])
    }

    fn receive_binary(
        &mut self,
        payload: &[u8],
        media: &mut MediaSession,
    ) -> Result<Vec<MediaWebSocketEvent>, MediaWebSocketError> {
        let stream = self
            .stream
            .as_ref()
            .ok_or(MediaWebSocketError::StreamNotStarted)?
            .clone();
        let media_config = media.config();
        if stream.codec != media_config.audio_codec
            || stream.sample_rate != media_config.rtp.clock_rate
        {
            return Err(MediaWebSocketError::MediaFormatMismatch);
        }
        if stream.direction == MediaDirection::In {
            return Ok(Vec::new());
        }
        if payload.is_empty() {
            return Err(MediaWebSocketError::InvalidMediaFrame);
        }
        if payload.len() > self.config.websocket.max_frame_bytes {
            return Err(MediaWebSocketError::AudioFrameTooLarge {
                actual: payload.len(),
                maximum: self.config.websocket.max_frame_bytes,
            });
        }
        let combined_len = self
            .inbound_remainder
            .len()
            .checked_add(payload.len())
            .ok_or(MediaWebSocketError::InvalidMediaFrame)?;
        if combined_len > self.config.websocket.max_message_bytes {
            return Err(MediaWebSocketError::AudioFrameTooLarge {
                actual: combined_len,
                maximum: self.config.websocket.max_message_bytes,
            });
        }
        self.inbound_remainder.extend_from_slice(payload);
        let mut events = Vec::new();
        while self.inbound_remainder.len() >= stream.frame_samples {
            let frame_bytes = self
                .inbound_remainder
                .drain(..stream.frame_samples)
                .collect::<Vec<_>>();
            let samples = media_core::decode(stream.codec, &frame_bytes);
            let timestamp = self.next_timestamp;
            self.next_timestamp = self
                .next_timestamp
                .wrapping_add(u32::try_from(samples.len()).unwrap_or(u32::MAX));
            let queued = media.push_from_ai(AudioFrame {
                timestamp,
                codec: stream.codec,
                sample_rate: stream.sample_rate,
                samples,
            });
            events.push(MediaWebSocketEvent::Audio {
                timestamp,
                samples: stream.frame_samples,
                queued,
            });
        }
        Ok(events)
    }
}

fn format_control(
    command: &MediaCommand,
    config: &MediaWebSocketConfig,
) -> Result<String, MediaWebSocketError> {
    let text = match command {
        MediaCommand::Start(start) => {
            let ptime = validate_media_start(start, config)?;
            format!(
                "MEDIA_START connection_id:{} channel_id:{} format:{} optimal_frame_size:{} ptime:{} sample_rate:{} direction:{}",
                start.connection_id,
                start.channel_id,
                format_name(start.codec),
                start.frame_samples,
                ptime,
                start.sample_rate,
                start.direction.as_str(),
            )
        }
        MediaCommand::Answer => "ANSWER".to_owned(),
        MediaCommand::Hangup => "HANGUP".to_owned(),
        MediaCommand::StartMediaBuffering => "START_MEDIA_BUFFERING".to_owned(),
        MediaCommand::StopMediaBuffering { correlation_id } => match correlation_id {
            None => "STOP_MEDIA_BUFFERING".to_owned(),
            Some(id) => {
                validate_identifier(id, config.max_identifier_bytes)?;
                format!("STOP_MEDIA_BUFFERING {id}")
            }
        },
        MediaCommand::PauseMedia => "PAUSE_MEDIA".to_owned(),
        MediaCommand::ContinueMedia => "CONTINUE_MEDIA".to_owned(),
        MediaCommand::SetMediaDirection(direction) => {
            format!("SET_MEDIA_DIRECTION {}", direction.as_str())
        }
        MediaCommand::ReportQueueDrained => "REPORT_QUEUE_DRAINED".to_owned(),
    };
    if text.len() > config.max_control_bytes {
        return Err(MediaWebSocketError::ControlTooLarge {
            actual: text.len(),
            maximum: config.max_control_bytes,
        });
    }
    Ok(text)
}

fn validate_media_start(
    start: &MediaStart,
    config: &MediaWebSocketConfig,
) -> Result<u32, MediaWebSocketError> {
    validate_identifier(&start.connection_id, config.max_identifier_bytes)?;
    validate_identifier(&start.channel_id, config.max_identifier_bytes)?;
    if start.sample_rate == 0 {
        return Err(MediaWebSocketError::InvalidControl);
    }
    if start.frame_samples == 0 || start.frame_samples > config.max_frame_samples {
        return Err(MediaWebSocketError::AudioFrameTooLarge {
            actual: start.frame_samples,
            maximum: config.max_frame_samples,
        });
    }
    let ptime = ((start.frame_samples as u128 * 1_000)
        .saturating_add(u128::from(start.sample_rate) - 1)
        / u128::from(start.sample_rate))
    .max(1);
    u32::try_from(ptime).map_err(|_| MediaWebSocketError::InvalidConfig)
}

fn parse_control(
    text: &str,
    config: &MediaWebSocketConfig,
) -> Result<MediaCommand, MediaWebSocketError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(MediaWebSocketError::InvalidControl);
    }
    let mut words = trimmed.split_whitespace();
    let command = words.next().ok_or(MediaWebSocketError::InvalidControl)?;
    match command {
        "MEDIA_START" => parse_start(words, config),
        "ANSWER" if words.next().is_none() => Ok(MediaCommand::Answer),
        "HANGUP" if words.next().is_none() => Ok(MediaCommand::Hangup),
        "START_MEDIA_BUFFERING" if words.next().is_none() => Ok(MediaCommand::StartMediaBuffering),
        "STOP_MEDIA_BUFFERING" => {
            let correlation_id = words.next().map(str::to_owned);
            if words.next().is_some() {
                return Err(MediaWebSocketError::InvalidControl);
            }
            if let Some(id) = correlation_id.as_deref() {
                validate_identifier(id, config.max_identifier_bytes)?;
            }
            Ok(MediaCommand::StopMediaBuffering { correlation_id })
        }
        "PAUSE_MEDIA" if words.next().is_none() => Ok(MediaCommand::PauseMedia),
        "CONTINUE_MEDIA" if words.next().is_none() => Ok(MediaCommand::ContinueMedia),
        "REPORT_QUEUE_DRAINED" if words.next().is_none() => Ok(MediaCommand::ReportQueueDrained),
        "SET_MEDIA_DIRECTION" => {
            let direction = words.next().ok_or(MediaWebSocketError::InvalidControl)?;
            if words.next().is_some() {
                return Err(MediaWebSocketError::InvalidControl);
            }
            Ok(MediaCommand::SetMediaDirection(MediaDirection::parse(
                direction,
            )?))
        }
        _ => Err(MediaWebSocketError::InvalidControl),
    }
}

fn parse_start<'a>(
    words: impl Iterator<Item = &'a str>,
    config: &MediaWebSocketConfig,
) -> Result<MediaCommand, MediaWebSocketError> {
    let mut connection_id = None;
    let mut channel_id = None;
    let mut codec = None;
    let mut frame_samples = None;
    let mut sample_rate = DEFAULT_SAMPLE_RATE;
    let mut sample_rate_seen = false;
    let mut ptime_seen = false;
    let mut direction = MediaDirection::Both;
    let mut direction_seen = false;
    for word in words {
        let (key, value) = word
            .split_once(':')
            .ok_or(MediaWebSocketError::InvalidControl)?;
        match key {
            "connection_id" if connection_id.is_none() => {
                validate_identifier(value, config.max_identifier_bytes)?;
                connection_id = Some(value.to_owned());
            }
            "connection_id" => return Err(MediaWebSocketError::InvalidControl),
            "channel_id" if channel_id.is_none() => {
                validate_identifier(value, config.max_identifier_bytes)?;
                channel_id = Some(value.to_owned());
            }
            "channel_id" => return Err(MediaWebSocketError::InvalidControl),
            "format" if codec.is_none() => codec = Some(parse_codec(value)?),
            "format" => return Err(MediaWebSocketError::InvalidControl),
            "optimal_frame_size" if frame_samples.is_none() => {
                frame_samples = Some(parse_frame_samples(value, config.max_frame_samples)?)
            }
            "optimal_frame_size" => return Err(MediaWebSocketError::InvalidControl),
            "ptime" if !ptime_seen => {
                let parsed = value
                    .parse::<u32>()
                    .map_err(|_| MediaWebSocketError::InvalidControl)?;
                if parsed == 0 {
                    return Err(MediaWebSocketError::InvalidControl);
                }
                ptime_seen = true;
            }
            "ptime" => return Err(MediaWebSocketError::InvalidControl),
            "sample_rate" if !sample_rate_seen => {
                sample_rate = value
                    .parse::<u32>()
                    .map_err(|_| MediaWebSocketError::InvalidControl)?;
                if sample_rate == 0 {
                    return Err(MediaWebSocketError::InvalidControl);
                }
                sample_rate_seen = true;
            }
            "sample_rate" => return Err(MediaWebSocketError::InvalidControl),
            "direction" if !direction_seen => {
                direction = MediaDirection::parse(value)?;
                direction_seen = true;
            }
            "direction" => return Err(MediaWebSocketError::InvalidControl),
            _ => {}
        }
    }
    let connection_id = connection_id.ok_or(MediaWebSocketError::MissingField("connection_id"))?;
    let channel_id = channel_id.ok_or(MediaWebSocketError::MissingField("channel_id"))?;
    let codec = codec.ok_or(MediaWebSocketError::MissingField("format"))?;
    let frame_samples =
        frame_samples.ok_or(MediaWebSocketError::MissingField("optimal_frame_size"))?;
    let start = MediaStart {
        connection_id,
        channel_id,
        codec,
        sample_rate,
        frame_samples,
        direction,
    };
    let _ = validate_media_start(&start, config)?;
    Ok(MediaCommand::Start(start))
}

fn validate_identifier(value: &str, maximum: usize) -> Result<(), MediaWebSocketError> {
    if value.is_empty() || value.len() > maximum || value.chars().any(char::is_whitespace) {
        return Err(MediaWebSocketError::InvalidControl);
    }
    Ok(())
}

fn parse_frame_samples(value: &str, maximum: usize) -> Result<usize, MediaWebSocketError> {
    let samples = value
        .parse::<usize>()
        .map_err(|_| MediaWebSocketError::InvalidControl)?;
    if samples == 0 || samples > maximum {
        return Err(MediaWebSocketError::AudioFrameTooLarge {
            actual: samples,
            maximum,
        });
    }
    Ok(samples)
}

fn parse_codec(value: &str) -> Result<AudioCodec, MediaWebSocketError> {
    if value.eq_ignore_ascii_case("ulaw") || value.eq_ignore_ascii_case("pcmu") {
        Ok(AudioCodec::Pcmu)
    } else if value.eq_ignore_ascii_case("alaw") || value.eq_ignore_ascii_case("pcma") {
        Ok(AudioCodec::Pcma)
    } else {
        Err(MediaWebSocketError::UnsupportedFormat)
    }
}

fn format_name(codec: AudioCodec) -> &'static str {
    match codec {
        AudioCodec::Pcmu => "ulaw",
        AudioCodec::Pcma => "alaw",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use media_core::{MediaBridgeConfig, MediaSessionConfig};
    use rtp::RtpSessionConfig;

    fn server_config() -> WebSocketConfig {
        WebSocketConfig {
            role: WebSocketRole::Server,
            max_frame_bytes: 256,
            max_message_bytes: 4_096,
            max_fragments: 4,
        }
    }

    fn media_session() -> MediaSession {
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

    fn masked(frame: &WebSocketFrame) -> Vec<u8> {
        frame
            .encode(
                WebSocketConfig {
                    role: WebSocketRole::Client,
                    ..server_config()
                },
                Some([1, 2, 3, 4]),
            )
            .unwrap()
    }

    #[test]
    fn decodes_masked_and_encodes_extended_frames() {
        let payload = vec![0x55; 130];
        let frame = WebSocketFrame::binary(payload.clone());
        let wire = masked(&frame);
        let (decoded, consumed) = WebSocketFrame::decode(&wire, server_config()).unwrap();
        assert_eq!(consumed, wire.len());
        assert_eq!(decoded, frame);

        let client = WebSocketConfig {
            role: WebSocketRole::Client,
            ..server_config()
        };
        let outbound = frame.encode(client, Some([9, 8, 7, 6])).unwrap();
        assert!(outbound[1] & 0x80 != 0);
        let (decoded, consumed) = WebSocketFrame::decode(&outbound, server_config()).unwrap();
        assert_eq!(consumed, outbound.len());
        assert_eq!(decoded, frame);
    }

    #[test]
    fn rejects_incomplete_or_wrong_masked_frames() {
        let wire = masked(&WebSocketFrame::text("hello"));
        assert_eq!(
            WebSocketFrame::decode(&wire[..wire.len() - 1], server_config()),
            Err(WebSocketError::Incomplete)
        );
        let unmasked = WebSocketFrame::text("hello")
            .encode(
                WebSocketConfig {
                    role: WebSocketRole::Server,
                    ..server_config()
                },
                None,
            )
            .unwrap();
        assert_eq!(
            WebSocketFrame::decode(&unmasked, server_config()),
            Err(WebSocketError::MaskRequired)
        );
    }

    #[test]
    fn reconstructs_fragments_and_allows_interleaved_ping() {
        let mut session = WebSocketSession::new(server_config()).unwrap();
        assert_eq!(
            session
                .receive_frame(WebSocketFrame::new(false, OpCode::Text, b"hel".to_vec()))
                .unwrap(),
            None
        );
        assert_eq!(
            session
                .receive_frame(WebSocketFrame::ping(vec![1]))
                .unwrap(),
            Some(WebSocketEvent::Ping(vec![1]))
        );
        assert_eq!(
            session
                .receive_frame(WebSocketFrame::new(
                    true,
                    OpCode::Continuation,
                    b"lo".to_vec()
                ))
                .unwrap(),
            Some(WebSocketEvent::Text("hello".to_owned()))
        );
    }

    #[test]
    fn direct_frames_recheck_control_and_close_bounds() {
        let mut session = WebSocketSession::new(server_config()).unwrap();
        assert_eq!(
            session.receive_frame(WebSocketFrame::new(false, OpCode::Ping, vec![])),
            Err(WebSocketError::FragmentedControl)
        );
        assert_eq!(
            session.receive_frame(WebSocketFrame::ping(vec![0; 126])),
            Err(WebSocketError::ControlFrameTooLarge)
        );
        assert_eq!(
            session.receive_frame(WebSocketFrame::binary(vec![0; 257])),
            Err(WebSocketError::FrameTooLarge {
                actual: 257,
                maximum: 256,
            })
        );
        assert_eq!(
            WebSocketFrame::close(CloseFrame {
                code: 1016,
                reason: String::new(),
            }),
            Err(WebSocketError::InvalidClose)
        );
    }

    #[test]
    fn parses_start_splits_binary_audio_and_queues_it() {
        let config = MediaWebSocketConfig {
            websocket: server_config(),
            max_frame_samples: 4,
            ..MediaWebSocketConfig::default()
        };
        let mut websocket = MediaWebSocketSession::new(config).unwrap();
        let mut media = media_session();
        let start = masked(&WebSocketFrame::text(
            "MEDIA_START connection_id:INCOMING channel_id:chan-1 format:ulaw optimal_frame_size:4",
        ));
        let (events, consumed) = websocket.receive(&start, &mut media).unwrap();
        assert_eq!(consumed, start.len());
        assert_eq!(
            events,
            vec![MediaWebSocketEvent::Command(MediaCommand::Start(
                MediaStart {
                    connection_id: "INCOMING".to_owned(),
                    channel_id: "chan-1".to_owned(),
                    codec: AudioCodec::Pcmu,
                    sample_rate: 8_000,
                    frame_samples: 4,
                    direction: MediaDirection::Both,
                }
            ))]
        );
        let binary = masked(&WebSocketFrame::binary(vec![0xff, 0xce, 0x4e, 0x00, 0xff]));
        let (events, _) = websocket.receive(&binary, &mut media).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            MediaWebSocketEvent::Audio {
                samples: 4,
                queued: PushOutcome::Accepted,
                ..
            }
        ));
        assert_eq!(media.stats().bridge.from_ai.depth, 1);
        assert_eq!(media.stats().bridge.from_ai.pushed, 1);
    }

    #[test]
    fn reset_releases_negotiated_and_partial_message_state() {
        let config = MediaWebSocketConfig {
            websocket: server_config(),
            max_frame_samples: 4,
            ..MediaWebSocketConfig::default()
        };
        let mut websocket = MediaWebSocketSession::new(config).unwrap();
        let mut media = media_session();
        websocket
            .receive_frame(
                WebSocketFrame::text(
                    "MEDIA_START connection_id:x channel_id:y format:ulaw optimal_frame_size:4",
                ),
                &mut media,
            )
            .unwrap();
        websocket
            .receive_frame(
                WebSocketFrame::new(false, OpCode::Binary, vec![0xff, 0xce]),
                &mut media,
            )
            .unwrap();

        let stream = websocket.reset();
        assert_eq!(stream.unwrap().connection_id, "x");
        assert!(websocket.stream().is_none());
        assert_eq!(
            websocket.receive_frame(
                WebSocketFrame::new(true, OpCode::Continuation, vec![0x4e, 0x00]),
                &mut media,
            ),
            Err(MediaWebSocketError::WebSocket(
                WebSocketError::UnexpectedContinuation
            ))
        );

        websocket
            .receive_frame(
                WebSocketFrame::text(
                    "MEDIA_START connection_id:new channel_id:z format:ulaw optimal_frame_size:4",
                ),
                &mut media,
            )
            .unwrap();
        let events = websocket
            .receive_frame(WebSocketFrame::binary(vec![0xff; 4]), &mut media)
            .unwrap();
        assert!(matches!(
            events.as_slice(),
            [MediaWebSocketEvent::Audio { timestamp: 0, .. }]
        ));
    }

    #[test]
    fn outbound_audio_encodes_and_consumes_ingress_queue() {
        let config = MediaWebSocketConfig {
            websocket: server_config(),
            max_frame_samples: 4,
            ..MediaWebSocketConfig::default()
        };
        let mut websocket = MediaWebSocketSession::new(config).unwrap();
        let mut media = media_session();
        websocket
            .receive_frame(
                WebSocketFrame::text(
                    "MEDIA_START connection_id:x channel_id:y format:ulaw optimal_frame_size:4",
                ),
                &mut media,
            )
            .unwrap();

        let mut sender = rtp::RtpSession::new(
            RtpSessionConfig {
                payload_type: 0,
                ..RtpSessionConfig::default()
            },
            1,
            1,
        )
        .unwrap();
        let wire = sender.send(&[0xff; 4], 4, false).unwrap();
        media.receive_rtp(&wire, std::time::Duration::ZERO).unwrap();
        assert_eq!(media.stats().bridge.to_ai.depth, 1);

        let outbound = websocket.next_audio(&mut media, None).unwrap().unwrap();
        assert_eq!(outbound[1] & 0x80, 0);
        let (frame, consumed) = WebSocketFrame::decode(
            &outbound,
            WebSocketConfig {
                role: WebSocketRole::Client,
                ..server_config()
            },
        )
        .unwrap();
        assert_eq!(consumed, outbound.len());
        assert_eq!(frame.opcode, OpCode::Binary);
        assert_eq!(frame.payload.len(), 4);
        assert_eq!(media.stats().bridge.to_ai.depth, 0);

        let client_config = MediaWebSocketConfig {
            websocket: WebSocketConfig {
                role: WebSocketRole::Client,
                ..server_config()
            },
            max_frame_samples: 4,
            ..MediaWebSocketConfig::default()
        };
        let mut client = MediaWebSocketSession::new(client_config).unwrap();
        let mut client_media = media_session();
        client
            .receive_frame(
                WebSocketFrame::text(
                    "MEDIA_START connection_id:x channel_id:y format:ulaw optimal_frame_size:4",
                ),
                &mut client_media,
            )
            .unwrap();
        let mut sender = rtp::RtpSession::new(
            RtpSessionConfig {
                payload_type: 0,
                ..RtpSessionConfig::default()
            },
            1,
            1,
        )
        .unwrap();
        let wire = sender.send(&[0xff; 4], 4, false).unwrap();
        client_media
            .receive_rtp(&wire, std::time::Duration::ZERO)
            .unwrap();
        let outbound = client
            .next_audio(&mut client_media, Some([4, 3, 2, 1]))
            .unwrap()
            .unwrap();
        assert_ne!(outbound[1] & 0x80, 0);
        let (frame, consumed) = WebSocketFrame::decode(&outbound, server_config()).unwrap();
        assert_eq!(consumed, outbound.len());
        assert_eq!(frame.opcode, OpCode::Binary);
        assert_eq!(frame.payload.len(), 4);
        assert_eq!(client_media.stats().bridge.to_ai.depth, 0);
    }

    #[test]
    fn rejects_ambiguous_or_invalid_media_start_controls() {
        let config = MediaWebSocketConfig {
            websocket: server_config(),
            max_frame_samples: 4,
            ..MediaWebSocketConfig::default()
        };
        let mut websocket = MediaWebSocketSession::new(config).unwrap();
        let mut media = media_session();
        for text in [
            "MEDIA_START connection_id:x channel_id:y format:ulaw optimal_frame_size:4 ptime:0",
            "MEDIA_START connection_id:x channel_id:y format:ulaw optimal_frame_size:4 direction:both direction:in",
            "MEDIA_START connection_id:x channel_id:y format:ulaw optimal_frame_size:4 sample_rate:8000 sample_rate:16000",
        ] {
            assert_eq!(
                websocket.receive_frame(WebSocketFrame::text(text), &mut media),
                Err(MediaWebSocketError::InvalidControl)
            );
            assert!(websocket.stream().is_none());
        }
        let oversized = MediaStart {
            connection_id: "x".repeat(257),
            channel_id: "y".to_owned(),
            codec: AudioCodec::Pcmu,
            sample_rate: 8_000,
            frame_samples: 4,
            direction: MediaDirection::Both,
        };
        assert_eq!(
            websocket.encode_control(&MediaCommand::Start(oversized), None),
            Err(MediaWebSocketError::InvalidControl)
        );
    }

    #[test]
    fn outbound_audio_stays_queued_when_format_is_wrong() {
        let config = MediaWebSocketConfig {
            websocket: server_config(),
            max_frame_samples: 4,
            ..MediaWebSocketConfig::default()
        };
        let mut websocket = MediaWebSocketSession::new(config).unwrap();
        let mut media = media_session();
        websocket
            .receive_frame(
                WebSocketFrame::text(
                    "MEDIA_START connection_id:x channel_id:y format:ulaw optimal_frame_size:4",
                ),
                &mut media,
            )
            .unwrap();
        let mut media = MediaSession::new(
            MediaSessionConfig {
                audio_codec: AudioCodec::Pcma,
                rtp: rtp::RtpSessionConfig {
                    payload_type: 8,
                    ..rtp::RtpSessionConfig::default()
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
        .unwrap();
        let mut sender = rtp::RtpSession::new(
            rtp::RtpSessionConfig {
                payload_type: 8,
                ..rtp::RtpSessionConfig::default()
            },
            1,
            1,
        )
        .unwrap();
        let wire = sender.send(&[0; 4], 4, false).unwrap();
        media.receive_rtp(&wire, std::time::Duration::ZERO).unwrap();
        assert_eq!(
            websocket.next_audio(&mut media, None),
            Err(MediaWebSocketError::MediaFormatMismatch)
        );
        assert_eq!(media.stats().bridge.to_ai.depth, 1);
    }

    #[test]
    fn direction_in_drops_application_binary_without_queue_growth() {
        let config = MediaWebSocketConfig {
            websocket: server_config(),
            max_frame_samples: 4,
            ..MediaWebSocketConfig::default()
        };
        let mut websocket = MediaWebSocketSession::new(config).unwrap();
        let mut media = media_session();
        websocket
            .receive_frame(
                WebSocketFrame::text(
                    "MEDIA_START connection_id:x channel_id:y format:ulaw optimal_frame_size:4 direction:in",
                ),
                &mut media,
            )
            .unwrap();
        let events = websocket
            .receive_frame(WebSocketFrame::binary(vec![0; 4]), &mut media)
            .unwrap();
        assert!(events.is_empty());
        assert_eq!(media.stats().bridge.from_ai.depth, 0);
    }
}
