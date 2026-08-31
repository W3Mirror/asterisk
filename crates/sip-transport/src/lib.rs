//! Bounded UDP/TCP SIP transport adapters and stream framing.

use std::{
    error::Error,
    fmt::{Display, Formatter},
    io::{self, Read, Write},
    net::{SocketAddr, TcpStream, UdpSocket},
    str,
    sync::Arc,
};

use rustls::{
    ClientConfig, ClientConnection, RootCertStore, ServerConfig, ServerConnection, StreamOwned,
    pki_types::ServerName,
};
use sip_parser::{ParseError, parse, serialize};
use sip_types::SipMessage;

const DEFAULT_MAX_MESSAGE_BYTES: usize = 65_535;
const TCP_READ_BYTES: usize = 8_192;

/// Builds a client configuration backed by the Mozilla WebPKI root set.
///
/// Applications that use a private carrier CA should build an equivalent
/// [`ClientConfig`] with their own [`RootCertStore`] and pass it to
/// [`TlsTransport::connect`] or [`TlsTransport::from_client_stream`].
#[must_use]
pub fn default_tls_client_config() -> Arc<ClientConfig> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    Arc::new(
        ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    )
}

/// Transport errors, including malformed SIP and bounded-frame failures.
#[derive(Debug)]
pub enum TransportError {
    /// The configured SIP frame limit is zero.
    InvalidMessageLimit,
    /// A serialized or received SIP frame exceeds the configured limit.
    MessageTooLarge { actual: usize, maximum: usize },
    /// A SIP transport header is not valid UTF-8.
    InvalidUtf8,
    /// A SIP `Content-Length` header is malformed or contradictory.
    InvalidContentLength,
    /// A TLS server name cannot be represented as a rustls server name.
    InvalidServerName,
    /// The underlying socket failed.
    Io(io::Error),
    /// SIP parsing failed after framing.
    Parse(ParseError),
    /// The TLS state machine rejected the connection.
    Tls(rustls::Error),
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
            Self::InvalidServerName => formatter.write_str("SIP TLS server name is invalid"),
            Self::Io(error) => Display::fmt(error, formatter),
            Self::Parse(error) => Display::fmt(error, formatter),
            Self::Tls(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for TransportError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Parse(error) => Some(error),
            Self::Tls(error) => Some(error),
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

impl From<rustls::Error> for TransportError {
    fn from(error: rustls::Error) -> Self {
        Self::Tls(error)
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

/// The role-specific rustls stream held by [`TlsTransport`].
#[derive(Debug)]
enum TlsStream {
    Client(StreamOwned<ClientConnection, TcpStream>),
    Server(StreamOwned<ServerConnection, TcpStream>),
}

impl Read for TlsStream {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Client(stream) => stream.read(bytes),
            Self::Server(stream) => stream.read(bytes),
        }
    }
}

impl Write for TlsStream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self {
            Self::Client(stream) => stream.write(bytes),
            Self::Server(stream) => stream.write(bytes),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Client(stream) => stream.flush(),
            Self::Server(stream) => stream.flush(),
        }
    }
}

/// A blocking SIP-over-TLS endpoint with the same bounded framing as TCP.
///
/// The TLS handshake is completed by the constructor. Client certificate
/// validation is controlled entirely by the supplied [`ClientConfig`]; the
/// server accepts no client certificate unless the supplied [`ServerConfig`]
/// explicitly requests one. After the handshake, SIP messages use the same
/// incremental `Content-Length` framing and per-frame limit as
/// [`TcpTransport`].
#[derive(Debug)]
pub struct TlsTransport {
    stream: Box<TlsStream>,
    framer: TcpFramer,
}

impl TlsTransport {
    /// Connects to a TLS peer and completes a client handshake.
    ///
    /// `server_name` is used for both SNI and certificate-name validation.
    /// The caller supplies the trust policy through `config`.
    ///
    /// # Errors
    ///
    /// Returns an error when the message limit, server name, TCP connection,
    /// or TLS handshake is invalid.
    pub fn connect(
        address: SocketAddr,
        server_name: &str,
        config: Arc<ClientConfig>,
        maximum: usize,
    ) -> Result<Self, TransportError> {
        let maximum = validate_limit(maximum)?;
        let stream = TcpStream::connect(address)?;
        Self::from_client_stream_with_limit(stream, server_name, config, maximum)
    }

    /// Wraps an already-connected stream as a TLS client and completes its
    /// handshake.
    ///
    /// # Errors
    ///
    /// Returns an error when the server name or TLS handshake is invalid.
    pub fn from_client_stream(
        stream: TcpStream,
        server_name: &str,
        config: Arc<ClientConfig>,
        maximum: usize,
    ) -> Result<Self, TransportError> {
        let maximum = validate_limit(maximum)?;
        Self::from_client_stream_with_limit(stream, server_name, config, maximum)
    }

    fn from_client_stream_with_limit(
        mut stream: TcpStream,
        server_name: &str,
        config: Arc<ClientConfig>,
        maximum: usize,
    ) -> Result<Self, TransportError> {
        let server_name = ServerName::try_from(server_name.to_owned())
            .map_err(|_| TransportError::InvalidServerName)?;
        let mut connection = ClientConnection::new(config, server_name)?;
        complete_client_handshake(&mut connection, &mut stream)?;
        Ok(Self {
            stream: Box::new(TlsStream::Client(StreamOwned::new(connection, stream))),
            framer: TcpFramer::new(maximum)?,
        })
    }

    /// Accepts an already-connected stream as a TLS server and completes its
    /// handshake.
    ///
    /// # Errors
    ///
    /// Returns an error when the TLS configuration or handshake is invalid.
    pub fn accept(
        stream: TcpStream,
        config: Arc<ServerConfig>,
        maximum: usize,
    ) -> Result<Self, TransportError> {
        let maximum = validate_limit(maximum)?;
        let mut stream = stream;
        let mut connection = ServerConnection::new(config)?;
        complete_server_handshake(&mut connection, &mut stream)?;
        Ok(Self {
            stream: Box::new(TlsStream::Server(StreamOwned::new(connection, stream))),
            framer: TcpFramer::new(maximum)?,
        })
    }

    /// Receives all complete SIP messages available in one TLS read.
    ///
    /// An empty vector indicates a clean TLS/TCP EOF. Partial frames remain
    /// buffered for the next call, just as with [`TcpTransport::recv`].
    pub fn recv(&mut self) -> Result<Vec<SipMessage>, TransportError> {
        let mut bytes = [0; TCP_READ_BYTES];
        let length = self.stream.read(&mut bytes)?;
        if length == 0 {
            return Ok(Vec::new());
        }
        self.framer.push(&bytes[..length])
    }

    /// Sends one bounded SIP message over the established TLS stream.
    ///
    /// # Errors
    ///
    /// Returns an error when serialization exceeds the configured frame
    /// limit or the TLS/TCP write fails.
    pub fn send(&mut self, message: &SipMessage) -> Result<(), TransportError> {
        let wire = serialize(message);
        if wire.len() > self.framer.maximum {
            return Err(TransportError::MessageTooLarge {
                actual: wire.len(),
                maximum: self.framer.maximum,
            });
        }
        self.stream.write_all(&wire)?;
        self.stream.flush()?;
        Ok(())
    }

    /// Sends a TLS `close_notify` alert and flushes it to the peer.
    ///
    /// The underlying TCP stream remains owned by this transport and is
    /// closed when the transport is dropped. A peer that calls [`Self::recv`]
    /// after this alert observes a clean EOF.
    ///
    /// # Errors
    ///
    /// Returns an error when the close notification cannot be flushed to the
    /// underlying TCP stream.
    pub fn shutdown(&mut self) -> Result<(), TransportError> {
        match self.stream.as_mut() {
            TlsStream::Client(stream) => stream.conn.send_close_notify(),
            TlsStream::Server(stream) => stream.conn.send_close_notify(),
        }
        self.stream.flush()?;
        Ok(())
    }

    /// Returns the connected TLS peer address.
    ///
    /// # Errors
    ///
    /// Returns the underlying socket-address error.
    pub fn peer_addr(&self) -> Result<SocketAddr, TransportError> {
        let stream = match self.stream.as_ref() {
            TlsStream::Client(stream) => stream.get_ref(),
            TlsStream::Server(stream) => stream.get_ref(),
        };
        Ok(stream.peer_addr()?)
    }

    /// Returns the number of bytes currently held by the SIP framer.
    #[must_use]
    pub fn buffered_len(&self) -> usize {
        self.framer.buffered_len()
    }
}

fn complete_client_handshake(
    connection: &mut ClientConnection,
    stream: &mut TcpStream,
) -> Result<(), TransportError> {
    while connection.is_handshaking() {
        connection.complete_io(stream)?;
    }
    Ok(())
}

fn complete_server_handshake(
    connection: &mut ServerConnection,
    stream: &mut TcpStream,
) -> Result<(), TransportError> {
    while connection.is_handshaking() {
        connection.complete_io(stream)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::generate_simple_self_signed;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
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

    fn tls_configs() -> (Arc<ClientConfig>, Arc<ServerConfig>) {
        let certificate = generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
        let certificate_der = CertificateDer::from(certificate.cert.der().to_vec());
        let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            certificate.key_pair.serialize_der(),
        ));

        let mut roots = RootCertStore::empty();
        roots.add(certificate_der.clone()).unwrap();
        let client = Arc::new(
            ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        );
        let server = Arc::new(
            ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(vec![certificate_der], private_key)
                .unwrap(),
        );
        (client, server)
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

    #[test]
    fn tls_transport_completes_validated_handshake_and_round_trips_framed_sip() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (client_config, server_config) = tls_configs();
        let server = thread::spawn(move || {
            let (stream, peer) = listener.accept().unwrap();
            let mut transport = TlsTransport::accept(stream, server_config, 1_024).unwrap();
            assert_eq!(transport.peer_addr().unwrap(), peer);
            let messages = loop {
                let messages = transport.recv().unwrap();
                if !messages.is_empty() {
                    break messages;
                }
            };
            transport.send(&options()).unwrap();
            messages
        });

        let mut client = TlsTransport::connect(address, "localhost", client_config, 1_024).unwrap();
        assert_eq!(client.peer_addr().unwrap(), address);
        client.send(&options()).unwrap();
        let response = loop {
            let messages = client.recv().unwrap();
            if !messages.is_empty() {
                break messages;
            }
        };
        assert_eq!(response, vec![options()]);
        assert_eq!(server.join().unwrap(), vec![options()]);
    }

    #[test]
    fn tls_transport_rejects_certificate_name_mismatch() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (client_config, server_config) = tls_configs();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            stream
                .set_write_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            TlsTransport::accept(stream, server_config, 1_024).is_err()
        });

        let error = TlsTransport::connect(address, "not-localhost", client_config, 1_024)
            .expect_err("certificate name mismatch must fail the client handshake");
        assert!(matches!(
            error,
            TransportError::Tls(_) | TransportError::Io(_)
        ));
        assert!(server.join().unwrap());
    }

    #[test]
    fn tls_transport_rejects_untrusted_certificate() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (_, server_config) = tls_configs();
        let client_config = Arc::new(
            ClientConfig::builder()
                .with_root_certificates(RootCertStore::empty())
                .with_no_client_auth(),
        );
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            stream
                .set_write_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            TlsTransport::accept(stream, server_config, 1_024).is_err()
        });

        let error = TlsTransport::connect(address, "localhost", client_config, 1_024)
            .expect_err("an unknown CA must fail the client handshake");
        assert!(matches!(
            error,
            TransportError::Tls(_) | TransportError::Io(_)
        ));
        assert!(server.join().unwrap());
    }

    #[test]
    fn tls_transport_preserves_frame_limit_and_reports_eof() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (client_config, server_config) = tls_configs();
        let wire_len = serialize(&options()).len();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut transport = TlsTransport::accept(stream, server_config, wire_len).unwrap();
            let messages = loop {
                let messages = transport.recv().unwrap();
                if !messages.is_empty() {
                    break messages;
                }
            };
            assert_eq!(messages, vec![options()]);
            transport
        });

        let mut client =
            TlsTransport::connect(address, "localhost", client_config, wire_len).unwrap();
        client.send(&options()).unwrap();
        let oversized = SipMessage::Request(SipRequest {
            method: SipMethod::Options,
            request_uri: "sip:peer@example.com".to_owned(),
            version: "SIP/2.0".to_owned(),
            headers: {
                let mut headers = Headers::new();
                headers.push("Content-Length", wire_len.to_string());
                headers
            },
            body: vec![b'x'; wire_len],
        });
        assert!(matches!(
            client.send(&oversized),
            Err(TransportError::MessageTooLarge { .. })
        ));
        client.shutdown().unwrap();
        let mut server = server.join().unwrap();
        assert!(server.recv().unwrap().is_empty());
        drop(client);
    }
}
