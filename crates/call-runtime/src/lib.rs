//! Blocking UDP/TCP adapters that drive the provider-neutral call engine.
//!
//! The runtime owns transport I/O and delegates all protocol state to
//! [`call_engine::CallEngine`]. It intentionally does not introduce an async
//! runtime, provider credentials, or Asterisk routing policy; an application
//! can wrap this boundary in its runtime of choice and retain Asterisk as a
//! fallback.

use std::{
    error::Error,
    fmt::{Display, Formatter},
    net::SocketAddr,
    time::Duration,
};

use call_api::CallCommand;
use call_core::CallId;
use call_engine::{CallEngine, EngineError, EngineOutput, SendAction};
use sip_security::SourceIpPolicy;
use sip_transaction::TransportReliability;
use sip_transport::{TcpTransport, TransportError, UdpTransport};
use sip_types::{SipMessage, SipRequest};

/// A blocking transport endpoint connected to one [`CallEngine`].
#[derive(Debug)]
pub enum RuntimeTransport {
    /// Datagram SIP transport. Each request/response carries its destination.
    Udp(UdpTransport),
    /// Stream SIP transport connected to one peer.
    Tcp {
        /// Connected TCP endpoint.
        transport: TcpTransport,
        /// Peer address used to validate engine action destinations.
        peer: SocketAddr,
    },
}

impl RuntimeTransport {
    fn reliability(&self) -> TransportReliability {
        match self {
            Self::Udp(_) => TransportReliability::Unreliable,
            Self::Tcp { .. } => TransportReliability::Reliable,
        }
    }

    fn receive(&mut self) -> Result<Vec<(SipMessage, SocketAddr)>, RuntimeError> {
        match self {
            Self::Udp(transport) => {
                let (message, source) = transport.recv()?;
                Ok(vec![(message, source)])
            }
            Self::Tcp { transport, peer } => {
                let messages = transport.recv()?;
                if messages.is_empty() && transport.buffered_len() == 0 {
                    return Err(RuntimeError::ConnectionClosed { peer: *peer });
                }
                Ok(messages
                    .into_iter()
                    .map(|message| (message, *peer))
                    .collect())
            }
        }
    }

    fn send(&mut self, action: &SendAction) -> Result<(), RuntimeError> {
        match self {
            Self::Udp(transport) => {
                let _ = transport.send_to(&action.message, action.destination)?;
                Ok(())
            }
            Self::Tcp { transport, peer } => {
                if action.destination != *peer {
                    return Err(RuntimeError::DestinationMismatch {
                        expected: *peer,
                        actual: action.destination,
                    });
                }
                transport.send(&action.message)?;
                Ok(())
            }
        }
    }

    /// Returns the local UDP address when this is a datagram endpoint.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::NotDatagram`] for a TCP endpoint or forwards a
    /// socket-address error from the UDP transport.
    pub fn local_addr(&self) -> Result<SocketAddr, RuntimeError> {
        match self {
            Self::Udp(transport) => Ok(transport.local_addr()?),
            Self::Tcp { .. } => Err(RuntimeError::NotDatagram),
        }
    }

    /// Returns the connected TCP peer when this is a stream endpoint.
    #[must_use]
    pub fn peer_addr(&self) -> Option<SocketAddr> {
        match self {
            Self::Udp(_) => None,
            Self::Tcp { peer, .. } => Some(*peer),
        }
    }
}

/// Errors raised while driving transport I/O and call-engine state.
#[derive(Debug)]
pub enum RuntimeError {
    /// The underlying SIP transport failed.
    Transport(TransportError),
    /// The call engine rejected a message or timer operation.
    Engine(EngineError),
    /// A connected stream cannot deliver an action to another destination.
    DestinationMismatch {
        /// Connected peer address.
        expected: SocketAddr,
        /// Engine-selected action destination.
        actual: SocketAddr,
    },
    /// The connected stream reached EOF with no buffered SIP frame.
    ConnectionClosed {
        /// Peer whose stream closed.
        peer: SocketAddr,
    },
    /// The observed peer address was rejected by the source-address policy.
    SourceAddressDenied {
        /// Observed peer address that failed the policy.
        source: SocketAddr,
    },
    /// A local-address query was made for a TCP endpoint.
    NotDatagram,
}

impl Display for RuntimeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(error) => Display::fmt(error, formatter),
            Self::Engine(error) => Display::fmt(error, formatter),
            Self::DestinationMismatch { expected, actual } => {
                write!(
                    formatter,
                    "TCP action destination {actual} does not match peer {expected}"
                )
            }
            Self::ConnectionClosed { peer } => write!(formatter, "TCP peer {peer} closed"),
            Self::SourceAddressDenied { source } => {
                write!(formatter, "source address {source} is not allowed")
            }
            Self::NotDatagram => formatter.write_str("endpoint does not expose a UDP address"),
        }
    }
}

impl Error for RuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Transport(error) => Some(error),
            Self::Engine(error) => Some(error),
            _ => None,
        }
    }
}

impl From<TransportError> for RuntimeError {
    fn from(error: TransportError) -> Self {
        Self::Transport(error)
    }
}

impl From<EngineError> for RuntimeError {
    fn from(error: EngineError) -> Self {
        Self::Engine(error)
    }
}

/// Ordered output emitted by one runtime operation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RuntimeOutput {
    actions: Vec<SendAction>,
    events: Vec<call_core::LifecycleEvent>,
}

impl RuntimeOutput {
    /// Returns outbound SIP actions after they were delivered to the transport.
    #[must_use]
    pub fn actions(&self) -> &[SendAction] {
        &self.actions
    }

    /// Returns lifecycle events emitted by the engine.
    #[must_use]
    pub fn events(&self) -> &[call_core::LifecycleEvent] {
        &self.events
    }

    fn append(&mut self, output: EngineOutput) {
        let (actions, events) = output.into_parts();
        self.actions.extend(actions);
        self.events.extend(events);
    }
}

/// A blocking message loop around [`CallEngine`] and UDP/TCP SIP transport.
#[derive(Debug)]
pub struct CallRuntime {
    engine: CallEngine,
    transport: RuntimeTransport,
    source_policy: SourceIpPolicy,
}

impl CallRuntime {
    /// Creates a UDP runtime. UDP transactions use retransmission timers.
    #[must_use]
    pub fn udp(engine: CallEngine, transport: UdpTransport) -> Self {
        Self::udp_with_source_policy(engine, transport, SourceIpPolicy::default())
    }

    /// Creates a UDP runtime with an explicit observed-source policy.
    #[must_use]
    pub fn udp_with_source_policy(
        engine: CallEngine,
        transport: UdpTransport,
        source_policy: SourceIpPolicy,
    ) -> Self {
        Self {
            engine,
            transport: RuntimeTransport::Udp(transport),
            source_policy,
        }
    }

    /// Creates a TCP runtime for an already-connected stream.
    #[must_use]
    pub fn tcp(engine: CallEngine, transport: TcpTransport, peer: SocketAddr) -> Self {
        Self::tcp_with_source_policy(engine, transport, peer, SourceIpPolicy::default())
    }

    /// Creates a TCP runtime with an explicit observed-source policy.
    #[must_use]
    pub fn tcp_with_source_policy(
        engine: CallEngine,
        transport: TcpTransport,
        peer: SocketAddr,
        source_policy: SourceIpPolicy,
    ) -> Self {
        Self {
            engine,
            transport: RuntimeTransport::Tcp { transport, peer },
            source_policy,
        }
    }

    /// Replaces the observed-source policy while preserving the runtime state.
    #[must_use]
    pub fn with_source_policy(mut self, source_policy: SourceIpPolicy) -> Self {
        self.source_policy = source_policy;
        self
    }

    /// Borrows the current call engine.
    #[must_use]
    pub fn engine(&self) -> &CallEngine {
        &self.engine
    }

    /// Borrows the current transport endpoint.
    #[must_use]
    pub fn transport(&self) -> &RuntimeTransport {
        &self.transport
    }

    /// Borrows the policy applied to observed UDP/TCP peer addresses.
    #[must_use]
    pub fn source_policy(&self) -> &SourceIpPolicy {
        &self.source_policy
    }

    /// Mutably borrows the call engine for application commands or negotiation.
    pub fn engine_mut(&mut self) -> &mut CallEngine {
        &mut self.engine
    }

    /// Starts an outbound INVITE and delivers it through the configured
    /// transport.
    ///
    /// # Errors
    ///
    /// Returns an error when the request is invalid, engine bounds are
    /// exhausted, or transport delivery fails. The engine is unchanged when
    /// delivery fails.
    pub fn originate(
        &mut self,
        request: SipRequest,
        destination: SocketAddr,
        now: Duration,
    ) -> Result<(CallId, RuntimeOutput), RuntimeError> {
        let mut working_engine = self.engine.clone();
        let (call_id, output) =
            working_engine.originate(request, destination, now, self.transport.reliability())?;
        let runtime_output = self.deliver_engine_output(output)?;
        self.engine = working_engine;
        Ok((call_id, runtime_output))
    }

    /// Applies one application call command and delivers any resulting action.
    ///
    /// # Errors
    ///
    /// Returns an error when the call command is invalid or transport delivery
    /// fails. The engine is unchanged when delivery fails.
    pub fn apply_call_command(
        &mut self,
        call_id: &CallId,
        command: CallCommand,
    ) -> Result<RuntimeOutput, RuntimeError> {
        let mut working_engine = self.engine.clone();
        let output = working_engine.apply_call_command(call_id, command)?;
        let runtime_output = self.deliver_engine_output(output)?;
        self.engine = working_engine;
        Ok(runtime_output)
    }

    /// Sends an application-controlled response to an inbound INVITE.
    ///
    /// # Errors
    ///
    /// Returns an error when the call is not backed by an inbound INVITE, the
    /// response is invalid, or transport delivery fails. The engine is
    /// unchanged when delivery fails.
    pub fn respond_to_invite(
        &mut self,
        call_id: &CallId,
        status_code: u16,
        reason: impl Into<String>,
        body: Vec<u8>,
        now: Duration,
    ) -> Result<RuntimeOutput, RuntimeError> {
        let mut working_engine = self.engine.clone();
        let output = working_engine.respond_to_invite(call_id, status_code, reason, body, now)?;
        let runtime_output = self.deliver_engine_output(output)?;
        self.engine = working_engine;
        Ok(runtime_output)
    }

    /// Receives and dispatches one blocking transport read.
    ///
    /// A TCP read may contain multiple SIP messages; all are dispatched in
    /// order before their outbound actions are written. A partial TCP frame
    /// returns an empty output and is retained by the framer for the next read.
    ///
    /// # Errors
    ///
    /// Returns an error when transport I/O, SIP framing, or engine validation
    /// fails.
    pub fn receive_once(&mut self, now: Duration) -> Result<RuntimeOutput, RuntimeError> {
        let messages = self.transport.receive()?;
        for (_, source) in &messages {
            if !self.source_policy.allows_socket(*source) {
                return Err(RuntimeError::SourceAddressDenied { source: *source });
            }
        }
        let reliability = self.transport.reliability();
        let mut working_engine = self.engine.clone();
        let mut output = RuntimeOutput::default();
        for (message, source) in messages {
            let engine_output = match message {
                SipMessage::Request(request) => {
                    working_engine.receive_request(source, request, now, reliability)?
                }
                SipMessage::Response(response) => working_engine.receive_response(response, now)?,
            };
            output.append(engine_output);
        }
        self.deliver(&output)?;
        self.engine = working_engine;
        Ok(output)
    }

    /// Polls call-engine timers and delivers any retransmissions or failures.
    ///
    /// # Errors
    ///
    /// Returns an error when engine timer processing or transport delivery
    /// fails.
    pub fn poll(&mut self, now: Duration) -> Result<RuntimeOutput, RuntimeError> {
        let mut working_engine = self.engine.clone();
        let mut output = RuntimeOutput::default();
        output.append(working_engine.poll(now)?);
        self.deliver(&output)?;
        self.engine = working_engine;
        Ok(output)
    }

    fn deliver(&mut self, output: &RuntimeOutput) -> Result<(), RuntimeError> {
        for action in &output.actions {
            self.transport.send(action)?;
        }
        Ok(())
    }

    fn deliver_engine_output(
        &mut self,
        output: EngineOutput,
    ) -> Result<RuntimeOutput, RuntimeError> {
        let mut runtime_output = RuntimeOutput::default();
        runtime_output.append(output);
        self.deliver(&runtime_output)?;
        Ok(runtime_output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use call_engine::EngineConfig;
    use sip_transport::{TcpTransport, UdpTransport};
    use sip_types::{Headers, SipMethod, SipRequest, SipResponse};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    fn options() -> SipMessage {
        let mut headers = Headers::new();
        headers.push("Via", "SIP/2.0/UDP client.invalid;branch=runtime-options");
        headers.push("From", "Alice <sip:alice@example.com>;tag=client-1");
        headers.push("To", "Bob <sip:bob@example.com>");
        headers.push("Call-ID", "runtime-options@example.com");
        headers.push("CSeq", "1 OPTIONS");
        SipMessage::Request(SipRequest {
            method: SipMethod::Options,
            request_uri: "sip:bob@example.com".to_owned(),
            version: "SIP/2.0".to_owned(),
            headers,
            body: Vec::new(),
        })
    }

    fn invite() -> SipRequest {
        let mut headers = Headers::new();
        headers.push("Via", "SIP/2.0/UDP client.invalid;branch=runtime-invite");
        headers.push("From", "Alice <sip:alice@example.com>;tag=client-1");
        headers.push("To", "Bob <sip:bob@example.com>");
        headers.push("Call-ID", "runtime-invite@example.com");
        headers.push("CSeq", "1 INVITE");
        headers.push("Contact", "<sip:alice@127.0.0.1:5060>");
        SipRequest {
            method: SipMethod::Invite,
            request_uri: "sip:bob@example.com".to_owned(),
            version: "SIP/2.0".to_owned(),
            headers,
            body: Vec::new(),
        }
    }

    fn inbound_invite() -> SipRequest {
        let mut headers = Headers::new();
        headers.push("Via", "SIP/2.0/UDP client.invalid;branch=runtime-inbound");
        headers.push("From", "Alice <sip:alice@example.com>;tag=client-2");
        headers.push("To", "Bob <sip:bob@example.com>");
        headers.push("Call-ID", "runtime-inbound@example.com");
        headers.push("CSeq", "1 INVITE");
        headers.push("Contact", "<sip:alice@127.0.0.1:5062>");
        SipRequest {
            method: SipMethod::Invite,
            request_uri: "sip:bob@example.com".to_owned(),
            version: "SIP/2.0".to_owned(),
            headers,
            body: Vec::new(),
        }
    }

    #[test]
    fn udp_runtime_dispatches_options_and_delivers_response() {
        let server_transport = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 2_048).unwrap();
        let server_address = server_transport.local_addr().unwrap();
        let client = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 2_048).unwrap();
        client.send_to(&options(), server_address).unwrap();
        let mut source_policy = SourceIpPolicy::default();
        source_policy.add_allow("127.0.0.1/8").unwrap();
        let mut runtime = CallRuntime::udp(
            CallEngine::new(EngineConfig::default()).unwrap(),
            server_transport,
        )
        .with_source_policy(source_policy);
        let output = runtime.receive_once(Duration::ZERO).unwrap();
        assert_eq!(output.actions().len(), 1);
        assert!(output.events().is_empty());
        let (response, _) = client.recv().unwrap();
        assert!(matches!(
            response,
            SipMessage::Response(SipResponse {
                status_code: 200,
                ..
            })
        ));
    }

    #[test]
    fn udp_runtime_rejects_source_before_engine_dispatch() {
        let server_transport = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 2_048).unwrap();
        let server_address = server_transport.local_addr().unwrap();
        let client = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 2_048).unwrap();
        client.send_to(&options(), server_address).unwrap();

        let mut policy = SourceIpPolicy::default();
        policy.add_allow("192.0.2.0/24").unwrap();
        let mut runtime = CallRuntime::udp_with_source_policy(
            CallEngine::new(EngineConfig::default()).unwrap(),
            server_transport,
            policy,
        );
        let error = runtime.receive_once(Duration::ZERO).unwrap_err();
        assert!(matches!(
            error,
            RuntimeError::SourceAddressDenied { source }
                if source.ip() == "127.0.0.1".parse::<std::net::IpAddr>().unwrap()
        ));
        assert!(runtime.engine().list(10).unwrap().is_empty());
    }

    #[test]
    fn tcp_runtime_dispatches_options_and_writes_framed_response() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let client_thread = thread::spawn(move || {
            let mut stream = TcpStream::connect(address).unwrap();
            stream
                .write_all(&sip_parser::serialize(&options()))
                .unwrap();
            let mut response = vec![0; 2_048];
            let length = stream.read(&mut response).unwrap();
            sip_parser::parse(&response[..length]).unwrap()
        });
        let (stream, peer) = listener.accept().unwrap();
        let transport = TcpTransport::from_stream(stream, 2_048).unwrap();
        let mut runtime = CallRuntime::tcp(
            CallEngine::new(EngineConfig::default()).unwrap(),
            transport,
            peer,
        );
        let output = runtime.receive_once(Duration::ZERO).unwrap();
        assert_eq!(output.actions().len(), 1);
        assert!(matches!(
            client_thread.join().unwrap(),
            SipMessage::Response(SipResponse {
                status_code: 200,
                ..
            })
        ));
    }

    #[test]
    fn tcp_runtime_rejects_connected_peer_before_engine_dispatch() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let client_thread = thread::spawn(move || {
            let mut stream = TcpStream::connect(address).unwrap();
            stream
                .write_all(&sip_parser::serialize(&options()))
                .unwrap();
        });
        let (stream, peer) = listener.accept().unwrap();
        let transport = TcpTransport::from_stream(stream, 2_048).unwrap();
        let mut policy = SourceIpPolicy::default();
        policy.add_allow("192.0.2.0/24").unwrap();
        let mut runtime = CallRuntime::tcp_with_source_policy(
            CallEngine::new(EngineConfig::default()).unwrap(),
            transport,
            peer,
            policy,
        );
        let error = runtime.receive_once(Duration::ZERO).unwrap_err();
        assert!(matches!(
            error,
            RuntimeError::SourceAddressDenied { source } if source == peer
        ));
        assert!(runtime.engine().list(10).unwrap().is_empty());
        client_thread.join().unwrap();
    }

    #[test]
    fn tcp_runtime_rejects_an_action_for_a_different_peer() {
        let expected = "127.0.0.1:5060".parse().unwrap();
        let actual = "127.0.0.1:5061".parse().unwrap();
        let error = RuntimeError::DestinationMismatch { expected, actual };
        assert!(error.to_string().contains("does not match"));
    }

    #[test]
    fn udp_runtime_originates_invite_and_delivers_application_response() {
        let server_transport = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 2_048).unwrap();
        let server_address = server_transport.local_addr().unwrap();
        let runtime_transport = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 2_048).unwrap();
        let mut runtime = CallRuntime::udp(
            CallEngine::new(EngineConfig::default()).unwrap(),
            runtime_transport,
        );
        let (call_id, output) = runtime
            .originate(invite(), server_address, Duration::ZERO)
            .unwrap();
        assert_eq!(output.actions().len(), 1);
        assert_eq!(output.events().len(), 2);
        assert_eq!(
            runtime.engine().snapshot(&call_id).unwrap().state,
            call_core::CallState::Inviting
        );
        let (message, _) = server_transport.recv().unwrap();
        assert!(matches!(
            message,
            SipMessage::Request(SipRequest {
                method: SipMethod::Invite,
                ..
            })
        ));

        // A runtime can also deliver an application-controlled response on
        // the same endpoint after an inbound INVITE has been received.
        let client = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 2_048).unwrap();
        let runtime_address = runtime.transport().local_addr().unwrap();
        client
            .send_to(&SipMessage::Request(inbound_invite()), runtime_address)
            .unwrap();
        let inbound = runtime.receive_once(Duration::ZERO).unwrap();
        let inbound_id = inbound.events()[0].call_id.clone();
        let (trying, _) = client.recv().unwrap();
        assert!(matches!(
            trying,
            SipMessage::Response(SipResponse {
                status_code: 100,
                ..
            })
        ));
        let response = runtime
            .respond_to_invite(&inbound_id, 200, "OK", Vec::new(), Duration::ZERO)
            .unwrap();
        assert_eq!(response.actions().len(), 1);
        let (message, _) = client.recv().unwrap();
        assert!(matches!(
            message,
            SipMessage::Response(SipResponse {
                status_code: 200,
                ..
            })
        ));
    }
}
