//! Bounded UDP/TCP SIP transport adapters and stream framing.

use std::{
    error::Error,
    fmt::{Display, Formatter},
    io::{self, Read, Write},
    net::{SocketAddr, TcpStream, UdpSocket},
    str,
};

use sip_parser::{ParseError, parse, serialize};
use sip_types::SipMessage;

const DEFAULT_MAX_MESSAGE_BYTES: usize = 65_535;
const TCP_READ_BYTES: usize = 8_192;

/// Transport errors, including malformed SIP and bounded-frame failures.
#[derive(Debug)]
pub enum TransportError {
    InvalidMessageLimit,
    MessageTooLarge { actual: usize, maximum: usize },
    InvalidUtf8,
    InvalidContentLength,
    Io(io::Error),
    Parse(ParseError),
}

impl Display for TransportError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidMessageLimit => {
                formatter.write_str("SIP transport message limit must be non-zero")
            }
            Self::MessageTooLarge { actual, maximum } => {
                write!(
                    formatter,
                    "SIP transport frame is {actual} bytes, maximum is {maximum}"
                )
            }
            Self::InvalidUtf8 => formatter.write_str("SIP transport headers are not valid UTF-8"),
            Self::InvalidContentLength => {
                formatter.write_str("SIP transport Content-Length is invalid")
            }
            Self::Io(error) => Display::fmt(error, formatter),
            Self::Parse(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for TransportError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Parse(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for TransportError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<ParseError> for TransportError {
    fn from(error: ParseError) -> Self {
        Self::Parse(error)
    }
}

fn validate_limit(maximum: usize) -> Result<usize, TransportError> {
    if maximum == 0 {
        Err(TransportError::InvalidMessageLimit)
    } else {
        Ok(maximum)
    }
}

fn declared_content_length(header_bytes: &[u8]) -> Result<usize, TransportError> {
    let header_text = str::from_utf8(header_bytes).map_err(|_| TransportError::InvalidUtf8)?;
    let mut declared = None;
    for line in header_text.split("\r\n").skip(1) {
        let Some((name, value)) = line.split_once(':') else {
            return Err(TransportError::InvalidContentLength);
        };
        if name.eq_ignore_ascii_case("Content-Length") {
            let parsed = value
                .trim()
                .parse::<usize>()
                .map_err(|_| TransportError::InvalidContentLength)?;
            if declared.is_some_and(|previous| previous != parsed) {
                return Err(TransportError::InvalidContentLength);
            }
            declared = Some(parsed);
        }
    }
    Ok(declared.unwrap_or(0))
}

/// Incremental SIP-over-TCP framer with a bounded accumulation buffer.
#[derive(Clone, Debug)]
pub struct TcpFramer {
    buffer: Vec<u8>,
    maximum: usize,
}

impl TcpFramer {
    pub fn new(maximum: usize) -> Result<Self, TransportError> {
        Ok(Self {
            buffer: Vec::new(),
            maximum: validate_limit(maximum)?,
        })
    }

    pub fn with_default_limit() -> Self {
        Self {
            buffer: Vec::new(),
            maximum: DEFAULT_MAX_MESSAGE_BYTES,
        }
    }

    /// Adds bytes and returns every complete SIP message now available.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<SipMessage>, TransportError> {
        let mut messages = Vec::new();
        let mut offset = 0;
        while offset < bytes.len() {
            self.drain_messages(&mut messages)?;
            let available = self.maximum.saturating_sub(self.buffer.len());
            if available == 0 {
                return Err(TransportError::MessageTooLarge {
                    actual: self
                        .buffer
                        .len()
                        .saturating_add(bytes.len().saturating_sub(offset)),
                    maximum: self.maximum,
                });
            }
            let take = available.min(bytes.len() - offset);
            self.buffer.extend_from_slice(&bytes[offset..offset + take]);
            offset += take;
        }
        self.drain_messages(&mut messages)?;
        Ok(messages)
    }

    fn drain_messages(&mut self, messages: &mut Vec<SipMessage>) -> Result<(), TransportError> {
        loop {
            let Some(delimiter) = self
                .buffer
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
            else {
                break;
            };
            let header_len = delimiter + 4;
            let body_len = declared_content_length(&self.buffer[..delimiter])?;
            let frame_len =
                header_len
                    .checked_add(body_len)
                    .ok_or(TransportError::MessageTooLarge {
                        actual: usize::MAX,
                        maximum: self.maximum,
                    })?;
            if frame_len > self.maximum {
                return Err(TransportError::MessageTooLarge {
                    actual: frame_len,
                    maximum: self.maximum,
                });
            }
            if self.buffer.len() < frame_len {
                break;
            }
            let frame = self.buffer.drain(..frame_len).collect::<Vec<_>>();
            messages.push(parse(&frame)?);
        }
        Ok(())
    }

    pub fn buffered_len(&self) -> usize {
        self.buffer.len()
    }
}

/// A bounded UDP SIP endpoint.
#[derive(Debug)]
pub struct UdpTransport {
    socket: UdpSocket,
    maximum: usize,
}

impl UdpTransport {
    pub fn bind(address: SocketAddr, maximum: usize) -> Result<Self, TransportError> {
        Ok(Self {
            socket: UdpSocket::bind(address)?,
            maximum: validate_limit(maximum)?,
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr, TransportError> {
        Ok(self.socket.local_addr()?)
    }

    pub fn recv(&self) -> Result<(SipMessage, SocketAddr), TransportError> {
        let mut buffer = vec![0; self.maximum.saturating_add(1)];
        let (length, source) = self.socket.recv_from(&mut buffer)?;
        if length > self.maximum {
            return Err(TransportError::MessageTooLarge {
                actual: length,
                maximum: self.maximum,
            });
        }
        Ok((parse(&buffer[..length])?, source))
    }

    pub fn send_to(
        &self,
        message: &SipMessage,
        destination: SocketAddr,
    ) -> Result<usize, TransportError> {
        let wire = serialize(message);
        if wire.len() > self.maximum {
            return Err(TransportError::MessageTooLarge {
                actual: wire.len(),
                maximum: self.maximum,
            });
        }
        Ok(self.socket.send_to(&wire, destination)?)
    }
}

/// A blocking TCP SIP endpoint with incremental Content-Length framing.
#[derive(Debug)]
pub struct TcpTransport {
    stream: TcpStream,
    framer: TcpFramer,
}

impl TcpTransport {
    pub fn connect(address: SocketAddr, maximum: usize) -> Result<Self, TransportError> {
        let stream = TcpStream::connect(address)?;
        Self::from_stream(stream, maximum)
    }

    pub fn from_stream(stream: TcpStream, maximum: usize) -> Result<Self, TransportError> {
        Ok(Self {
            stream,
            framer: TcpFramer::new(maximum)?,
        })
    }

    pub fn recv(&mut self) -> Result<Vec<SipMessage>, TransportError> {
        let mut bytes = [0; TCP_READ_BYTES];
        let length = self.stream.read(&mut bytes)?;
        if length == 0 {
            return Ok(Vec::new());
        }
        self.framer.push(&bytes[..length])
    }

    pub fn send(&mut self, message: &SipMessage) -> Result<(), TransportError> {
        let wire = serialize(message);
        if wire.len() > self.framer.maximum {
            return Err(TransportError::MessageTooLarge {
                actual: wire.len(),
                maximum: self.framer.maximum,
            });
        }
        self.stream.write_all(&wire)?;
        Ok(())
    }

    pub fn buffered_len(&self) -> usize {
        self.framer.buffered_len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sip_types::{Headers, SipMethod, SipRequest};
    use std::{net::TcpListener, thread};

    fn options() -> SipMessage {
        let mut headers = Headers::new();
        headers.push("Content-Length", "0");
        SipMessage::Request(SipRequest {
            method: SipMethod::Options,
            request_uri: "sip:peer@example.com".to_owned(),
            version: "SIP/2.0".to_owned(),
            headers,
            body: Vec::new(),
        })
    }

    #[test]
    fn tcp_framer_handles_partial_and_multiple_messages() {
        let wire = serialize(&options());
        let mut framer = TcpFramer::new(1_024).unwrap();
        assert!(framer.push(&wire[..8]).unwrap().is_empty());
        assert_eq!(framer.buffered_len(), 8);
        let mut combined = wire[8..].to_vec();
        combined.extend_from_slice(&wire);
        let messages = framer.push(&combined).unwrap();
        assert_eq!(messages, vec![options(), options()]);
        assert_eq!(framer.buffered_len(), 0);

        // The limit is per SIP frame, not per read buffer: two complete
        // frames may arrive in one TCP read without being rejected.
        let mut per_frame = TcpFramer::new(wire.len()).unwrap();
        let mut two_frames = wire.clone();
        two_frames.extend_from_slice(&wire);
        assert_eq!(
            per_frame.push(&two_frames).unwrap(),
            vec![options(), options()]
        );
    }

    #[test]
    fn tcp_framer_waits_for_declared_body_and_rejects_mismatch() {
        let header = b"OPTIONS sip:peer@example.com SIP/2.0\r\nContent-Length: 4\r\n\r\n";
        let mut framer = TcpFramer::new(128).unwrap();
        assert!(framer.push(header).unwrap().is_empty());
        assert_eq!(framer.buffered_len(), header.len());
        assert!(framer.push(b"ping").is_ok());
        let mut invalid = TcpFramer::new(128).unwrap();
        assert!(matches!(
            invalid.push(b"OPTIONS sip:x SIP/2.0\r\nContent-Length: nope\r\n\r\n"),
            Err(TransportError::InvalidContentLength)
        ));
    }

    #[test]
    fn udp_transport_round_trips_a_message() {
        let receiver = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 1_024).unwrap();
        let sender = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 1_024).unwrap();
        sender
            .send_to(&options(), receiver.local_addr().unwrap())
            .unwrap();
        let (message, source) = receiver.recv().unwrap();
        assert_eq!(message, options());
        assert_eq!(source, sender.local_addr().unwrap());
    }

    #[test]
    fn tcp_transport_round_trips_a_message() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut transport = TcpTransport::from_stream(stream, 1_024).unwrap();
            loop {
                let messages = transport.recv().unwrap();
                if !messages.is_empty() {
                    return messages;
                }
            }
        });
        let mut client = TcpTransport::connect(address, 1_024).unwrap();
        client.send(&options()).unwrap();
        assert_eq!(server.join().unwrap(), vec![options()]);
    }
}
