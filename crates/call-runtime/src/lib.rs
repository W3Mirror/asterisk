//! Blocking UDP/TCP/TLS adapters that drive the provider-neutral call engine.
//!
//! The runtime owns transport I/O and delegates all protocol state to
//! [`call_engine::CallEngine`]. It intentionally does not introduce an async
//! runtime, provider credentials, or Asterisk routing policy; an application
//! can wrap this boundary in its runtime of choice and retain Asterisk as a
//! fallback.

mod media_bridge;

pub use media_bridge::{
    HumanAudioPlayout, HumanMediaBridgeError, HumanMediaBridgeRuntime, HumanMediaDirection,
    HumanMediaForward,
};

use std::{
    collections::HashMap,
    error::Error,
    fmt::{Display, Formatter},
    net::SocketAddr,
    time::Duration,
};

use call_api::{AuditRecord, AuthenticatedPrincipal, CallCommand};
use call_bridge::{BridgeError, BridgeEvent, BridgeRegistry, BridgeState};
use call_core::{BridgeId, CallEventKind, CallId, CommandId, LegId, LifecycleEvent};
use call_engine::{
    CallDiagnostics, CallEngine, EngineError, EngineHealth, EngineMetrics, EngineOutput,
    RestartHandoff as EngineRestartHandoff, SendAction,
};
use provider_routing::{
    AuthenticationPolicy, EngineTarget, ProviderRouteTable, RouteController, RouteDecision,
    RouteMatch, SignalingTransport,
};
use sip_auth::DigestCredentials;
use sip_security::SourceIpPolicy;
use sip_transaction::TransportReliability;
use sip_transport::{TcpTransport, TlsTransport, TransportError, UdpTransport};
use sip_types::{SipMessage, SipRequest, SipResponse};

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
    /// TLS-encrypted SIP stream connected to one peer.
    Tls {
        /// Connected TLS endpoint.
        transport: TlsTransport,
        /// Peer address used to validate engine action destinations.
        peer: SocketAddr,
    },
}

impl RuntimeTransport {
    fn reliability(&self) -> TransportReliability {
        match self {
            Self::Udp(_) => TransportReliability::Unreliable,
            Self::Tcp { .. } | Self::Tls { .. } => TransportReliability::Reliable,
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
            Self::Tls { transport, peer } => {
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
            Self::Tls { transport, peer } => {
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
            Self::Tcp { .. } | Self::Tls { .. } => Err(RuntimeError::NotDatagram),
        }
    }

    /// Returns the connected TCP peer when this is a stream endpoint.
    #[must_use]
    pub fn peer_addr(&self) -> Option<SocketAddr> {
        match self {
            Self::Udp(_) => None,
            Self::Tcp { peer, .. } | Self::Tls { peer, .. } => Some(*peer),
        }
    }
}

/// Resolves one provider credential reference at the moment an authenticated
/// retry is required.
///
/// Implementations should fetch the current secret-store value on every call
/// so credential rotation does not require rebuilding the runtime. Returned
/// credentials are consumed only for the current Digest calculation and are
/// never retained by [`CallRuntime`].
pub trait DigestCredentialResolver {
    /// Returns the current credentials for a secret-opaque provider reference.
    fn resolve(&mut self, credential_ref: &str) -> Option<DigestCredentials>;
}

impl<F> DigestCredentialResolver for F
where
    F: FnMut(&str) -> Option<DigestCredentials>,
{
    fn resolve(&mut self, credential_ref: &str) -> Option<DigestCredentials> {
        self(credential_ref)
    }
}

/// Resolves a provider signaling host at the moment an outbound call is
/// originated.
///
/// The runtime deliberately leaves DNS, service discovery, and address
/// caching to the embedding application. A resolver is called only after the
/// route selects the Rust engine and the transport is compatible.
pub trait ProviderAddressResolver {
    /// Returns one destination address for a provider signaling endpoint.
    fn resolve(&mut self, host: &str, port: u16) -> Option<SocketAddr>;
}

impl<F> ProviderAddressResolver for F
where
    F: FnMut(&str, u16) -> Option<SocketAddr>,
{
    fn resolve(&mut self, host: &str, port: u16) -> Option<SocketAddr> {
        self(host, port)
    }
}

/// Errors raised while driving transport I/O and call-engine state.
#[derive(Debug)]
pub enum RuntimeError {
    /// The underlying SIP transport failed.
    Transport(TransportError),
    /// The call engine rejected a message or timer operation.
    Engine(EngineError),
    /// Runtime bridge orchestration rejected a human-leg transition.
    Bridge(BridgeError),
    /// A human-leg operation requires an attached bridge registry.
    BridgeRegistryNotConfigured,
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
    /// Provider routing selected the mandatory Asterisk fallback.
    ProviderRouteFallback {
        /// Why the route was selected.
        matched_by: RouteMatch,
    },
    /// The selected provider profile is not supported by this transport.
    ProviderTransportUnsupported,
    /// The provider signaling endpoint could not be resolved.
    ProviderAddressUnavailable,
}

impl Display for RuntimeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(error) => Display::fmt(error, formatter),
            Self::Engine(error) => Display::fmt(error, formatter),
            Self::Bridge(error) => Display::fmt(error, formatter),
            Self::BridgeRegistryNotConfigured => {
                formatter.write_str("call runtime has no bridge registry")
            }
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
            Self::ProviderRouteFallback { matched_by } => {
                write!(
                    formatter,
                    "provider route selected Asterisk fallback ({matched_by:?})"
                )
            }
            Self::ProviderTransportUnsupported => {
                formatter.write_str("provider signaling transport is not supported by this runtime")
            }
            Self::ProviderAddressUnavailable => {
                formatter.write_str("provider signaling address is unavailable")
            }
        }
    }
}

impl Error for RuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Transport(error) => Some(error),
            Self::Engine(error) => Some(error),
            Self::Bridge(error) => Some(error),
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

impl From<BridgeError> for RuntimeError {
    fn from(error: BridgeError) -> Self {
        Self::Bridge(error)
    }
}

/// Ordered output emitted by one runtime operation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RuntimeOutput {
    actions: Vec<SendAction>,
    events: Vec<LifecycleEvent>,
    bridge_events: Vec<BridgeEvent>,
}

impl RuntimeOutput {
    /// Returns outbound SIP actions after they were delivered to the transport.
    #[must_use]
    pub fn actions(&self) -> &[SendAction] {
        &self.actions
    }

    /// Returns lifecycle events emitted by the engine.
    #[must_use]
    pub fn events(&self) -> &[LifecycleEvent] {
        &self.events
    }

    /// Returns bridge transitions emitted while processing call lifecycle.
    #[must_use]
    pub fn bridge_events(&self) -> &[BridgeEvent] {
        &self.bridge_events
    }

    fn append(&mut self, output: EngineOutput) {
        let (actions, events) = output.into_parts();
        self.actions.extend(actions);
        self.events.extend(events);
    }
}

/// Bounded result of preparing a runtime for a controlled process restart.
///
/// The report contains only application call and lifecycle identifiers plus
/// bridge events. SIP message bodies, network addresses, and credentials are
/// intentionally excluded so it can be persisted by a restart supervisor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeRestartHandoff {
    engine: EngineRestartHandoff,
    bridge_events: Vec<BridgeEvent>,
}

impl RuntimeRestartHandoff {
    /// Returns application calls that were active when the handoff started.
    #[must_use]
    pub fn terminated_calls(&self) -> &[CallId] {
        self.engine.terminated_calls()
    }

    /// Returns ordered call lifecycle events that must be delivered or
    /// persisted before replacing the process.
    #[must_use]
    pub fn events(&self) -> &[LifecycleEvent] {
        self.engine.events()
    }

    /// Returns ordered bridge events emitted while ending bridge forwarding.
    #[must_use]
    pub fn bridge_events(&self) -> &[BridgeEvent] {
        &self.bridge_events
    }

    /// Returns runtime health after the handoff has terminalized calls.
    #[must_use]
    pub const fn health(&self) -> EngineHealth {
        self.engine.health()
    }

    /// Returns whether no active calls required terminalization.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.engine.is_empty()
    }
}

/// A blocking message loop around [`CallEngine`] and UDP/TCP/TLS SIP transport.
#[derive(Debug)]
pub struct CallRuntime {
    engine: CallEngine,
    transport: RuntimeTransport,
    source_policy: SourceIpPolicy,
    bridges: Option<BridgeRegistry>,
    provider_authentication: HashMap<String, (CallId, AuthenticationPolicy)>,
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
            bridges: None,
            provider_authentication: HashMap::new(),
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
            bridges: None,
            provider_authentication: HashMap::new(),
        }
    }

    /// Creates a TLS runtime for an already-connected and handshaken stream.
    #[must_use]
    pub fn tls(engine: CallEngine, transport: TlsTransport, peer: SocketAddr) -> Self {
        Self::tls_with_source_policy(engine, transport, peer, SourceIpPolicy::default())
    }

    /// Creates a TLS runtime with an explicit observed peer-address policy.
    #[must_use]
    pub fn tls_with_source_policy(
        engine: CallEngine,
        transport: TlsTransport,
        peer: SocketAddr,
        source_policy: SourceIpPolicy,
    ) -> Self {
        Self {
            engine,
            transport: RuntimeTransport::Tls { transport, peer },
            source_policy,
            bridges: None,
            provider_authentication: HashMap::new(),
        }
    }

    /// Replaces the observed-source policy while preserving the runtime state.
    #[must_use]
    pub fn with_source_policy(mut self, source_policy: SourceIpPolicy) -> Self {
        self.source_policy = source_policy;
        self
    }

    /// Attaches bounded bridge state for runtime human-leg orchestration.
    #[must_use]
    pub fn with_bridge_registry(mut self, bridges: BridgeRegistry) -> Self {
        self.bridges = Some(bridges);
        self
    }

    /// Borrows the current call engine.
    #[must_use]
    pub fn engine(&self) -> &CallEngine {
        &self.engine
    }

    /// Returns a redaction-safe diagnostic view for one retained call.
    pub fn diagnostics(&self, call_id: &CallId) -> Result<CallDiagnostics, RuntimeError> {
        Ok(self.engine.diagnostics(call_id)?)
    }

    /// Returns one diagnostic view after verifying control-plane read access.
    pub fn diagnostics_authorized(
        &self,
        principal: &AuthenticatedPrincipal,
        call_id: &CallId,
    ) -> Result<CallDiagnostics, RuntimeError> {
        Ok(self.engine.diagnostics_authorized(principal, call_id)?)
    }

    /// Returns deterministic redaction-safe diagnostics ordered by call ID.
    pub fn list_diagnostics(&self, limit: usize) -> Result<Vec<CallDiagnostics>, RuntimeError> {
        Ok(self.engine.list_diagnostics(limit)?)
    }

    /// Lists diagnostic views after verifying control-plane read access.
    pub fn list_diagnostics_authorized(
        &self,
        principal: &AuthenticatedPrincipal,
        limit: usize,
    ) -> Result<Vec<CallDiagnostics>, RuntimeError> {
        Ok(self.engine.list_diagnostics_authorized(principal, limit)?)
    }

    /// Returns aggregate runtime, call, and signaling metrics.
    #[must_use]
    pub fn metrics(&self) -> EngineMetrics {
        self.engine.metrics()
    }

    /// Returns liveness and capacity-aware admission readiness for the runtime.
    #[must_use]
    pub fn health(&self) -> EngineHealth {
        self.engine.health()
    }

    /// Stops admitting new calls while allowing existing dialogs and
    /// transactions to continue draining to completion.
    pub fn begin_drain(&mut self) {
        self.engine.begin_drain();
    }

    /// Resumes admitting new calls after a drain or restart handoff.
    pub fn resume(&mut self) {
        self.engine.resume();
    }

    /// Returns whether the runtime is currently draining new call admission.
    #[must_use]
    pub fn is_draining(&self) -> bool {
        self.engine.is_draining()
    }

    /// Drains bounded control-plane audit records after verifying read access.
    ///
    /// # Errors
    ///
    /// Returns an error when the principal lacks `calls:read` or when `limit`
    /// is zero.
    pub fn drain_audit_records(
        &mut self,
        principal: &AuthenticatedPrincipal,
        limit: usize,
    ) -> Result<Vec<AuditRecord>, RuntimeError> {
        let mut working_engine = self.engine.clone();
        let records = working_engine.drain_audit_records(principal, limit)?;
        self.engine = working_engine;
        Ok(records)
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

    /// Borrows the attached bridge registry, if runtime orchestration is enabled.
    #[must_use]
    pub fn bridge_registry(&self) -> Option<&BridgeRegistry> {
        self.bridges.as_ref()
    }

    /// Mutably borrows the attached bridge registry.
    pub fn bridge_registry_mut(&mut self) -> Option<&mut BridgeRegistry> {
        self.bridges.as_mut()
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
        let runtime_output = self.commit_engine_output(working_engine, output)?;
        Ok((call_id, runtime_output))
    }

    /// Starts an outbound INVITE after verifying control-plane originate
    /// permission, then delivers it through the configured transport.
    pub fn originate_authorized(
        &mut self,
        principal: &AuthenticatedPrincipal,
        request: SipRequest,
        destination: SocketAddr,
        now: Duration,
    ) -> Result<(CallId, RuntimeOutput), RuntimeError> {
        let mut working_engine = self.engine.clone();
        let result = working_engine.originate_authorized(
            principal,
            request,
            destination,
            now,
            self.transport.reliability(),
        );
        match result {
            Ok((call_id, output)) => {
                let runtime_output = self.commit_engine_output(working_engine, output)?;
                Ok((call_id, runtime_output))
            }
            Err(error) => {
                // Authorization and validation failures intentionally emit an
                // audit record without producing wire output. Preserve that
                // record while leaving all other committed runtime state alone.
                self.engine = working_engine;
                Err(error.into())
            }
        }
    }

    /// Starts an outbound call only when the provider route explicitly
    /// selects the Rust engine.
    ///
    /// The route table's default and fallback targets are fail-closed: this
    /// method never originates a Rust call for an Asterisk route. Address
    /// resolution is deferred until after that gate and after checking that
    /// this blocking runtime supports the profile's signaling transport.
    ///
    /// # Errors
    ///
    /// Returns an error when routing selects Asterisk, the provider transport
    /// is unsupported, address resolution fails, or ordinary origination or
    /// delivery fails. No engine state or wire action is committed on error.
    pub fn originate_with_provider_route<R>(
        &mut self,
        routes: &ProviderRouteTable,
        provider_id: &str,
        request: SipRequest,
        resolver: &mut R,
        now: Duration,
    ) -> Result<(CallId, RuntimeOutput), RuntimeError>
    where
        R: ProviderAddressResolver + ?Sized,
    {
        let decision = routes.route_outbound(provider_id);
        self.originate_with_route_decision(routes, &decision, request, resolver, now)
    }

    /// Starts an outbound call through a generation-checked deployment route
    /// controller.
    ///
    /// The controller starts fail-closed on Asterisk and can be switched to
    /// explicitly Rust-targeted profiles only by the deployment supervisor.
    /// Rolling the controller back therefore prevents address resolution and
    /// wire delivery before this runtime mutates any call state.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::originate_with_provider_route`]. A
    /// controller in its Asterisk rollback state returns
    /// [`RuntimeError::ProviderRouteFallback`] without invoking `resolver`.
    pub fn originate_with_route_controller<R>(
        &mut self,
        routes: &RouteController,
        provider_id: &str,
        request: SipRequest,
        resolver: &mut R,
        now: Duration,
    ) -> Result<(CallId, RuntimeOutput), RuntimeError>
    where
        R: ProviderAddressResolver + ?Sized,
    {
        let decision = routes.route_outbound(provider_id);
        self.originate_with_route_decision(routes.table(), &decision, request, resolver, now)
    }

    fn originate_with_route_decision<R>(
        &mut self,
        routes: &ProviderRouteTable,
        decision: &RouteDecision,
        request: SipRequest,
        resolver: &mut R,
        now: Duration,
    ) -> Result<(CallId, RuntimeOutput), RuntimeError>
    where
        R: ProviderAddressResolver + ?Sized,
    {
        if self.engine.is_draining() {
            return Err(RuntimeError::Engine(EngineError::Draining));
        }
        if decision.target() != EngineTarget::Rust {
            return Err(RuntimeError::ProviderRouteFallback {
                matched_by: decision.matched_by(),
            });
        }
        let profile_id = decision
            .profile_id()
            .ok_or(RuntimeError::ProviderRouteFallback {
                matched_by: decision.matched_by(),
            })?;
        let profile = routes
            .profile(profile_id)
            .ok_or(RuntimeError::ProviderRouteFallback {
                matched_by: decision.matched_by(),
            })?;
        if !provider_transport_matches(&self.transport, profile.signaling_transport()) {
            return Err(RuntimeError::ProviderTransportUnsupported);
        }
        let destination = resolver
            .resolve(profile.signaling_host(), profile.signaling_port())
            .ok_or(RuntimeError::ProviderAddressUnavailable)?;
        let sip_call_id = request.headers.get("Call-ID").map(str::to_owned);
        let authentication = profile.authentication().clone();
        let result = self.originate(request, destination, now);
        if let (Ok((call_id, _)), Some(sip_call_id)) = (&result, sip_call_id) {
            self.provider_authentication
                .insert(sip_call_id, (call_id.clone(), authentication));
        }
        result
    }

    /// Starts an outbound human INVITE and atomically marks a bridge as
    /// connecting that new call leg before any wire action is delivered.
    ///
    /// # Errors
    ///
    /// Returns an error when no bridge registry is attached, signaling or
    /// bridge validation fails, or transport delivery fails. The call engine
    /// and bridge registry remain unchanged on every error.
    pub fn originate_human_leg(
        &mut self,
        bridge_id: &BridgeId,
        leg_id: LegId,
        request: SipRequest,
        destination: SocketAddr,
        now: Duration,
    ) -> Result<(CallId, RuntimeOutput), RuntimeError> {
        let mut working_bridges = self
            .bridges
            .clone()
            .ok_or(RuntimeError::BridgeRegistryNotConfigured)?;
        let mut working_engine = self.engine.clone();
        let (call_id, engine_output) =
            working_engine.originate(request, destination, now, self.transport.reliability())?;
        let _ = working_bridges.begin_human(bridge_id, call_id.clone(), leg_id)?;
        let mut output = RuntimeOutput::default();
        output.append(engine_output);
        output
            .bridge_events
            .extend(working_bridges.drain_events(usize::MAX)?);
        self.deliver(&output)?;
        self.engine = working_engine;
        self.bridges = Some(working_bridges);
        Ok((call_id, output))
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
        self.commit_engine_output(working_engine, output)
    }

    /// Applies one call command after verifying control-plane permission and
    /// delivers any resulting transport action.
    pub fn apply_authorized_call_command(
        &mut self,
        principal: &AuthenticatedPrincipal,
        call_id: &CallId,
        command: CallCommand,
    ) -> Result<RuntimeOutput, RuntimeError> {
        let mut working_engine = self.engine.clone();
        match working_engine.apply_authorized_call_command(principal, call_id, command) {
            Ok(output) => self.commit_engine_output(working_engine, output),
            Err(error) => {
                // Keep the audit-only mutation from a rejected authorized
                // operation; no lifecycle event or transport action exists to
                // commit on this path.
                self.engine = working_engine;
                Err(error.into())
            }
        }
    }

    /// Applies an application command with a bounded idempotency key and
    /// delivers its lifecycle result to the transport boundary.
    ///
    /// Retried keys replay the original event without mutating call state or
    /// producing duplicate events in the engine's pending queue.
    pub fn apply_idempotent_call_command(
        &mut self,
        call_id: &CallId,
        command: CallCommand,
        command_id: CommandId,
    ) -> Result<RuntimeOutput, RuntimeError> {
        let mut working_engine = self.engine.clone();
        let output = working_engine.apply_idempotent_call_command(call_id, command, command_id)?;
        self.commit_engine_output(working_engine, output)
    }

    /// Applies an idempotent call command after verifying control-plane
    /// permission. Authorization precedes retry-key lookup and transport
    /// delivery remains atomic with engine state.
    pub fn apply_authorized_idempotent_call_command(
        &mut self,
        principal: &AuthenticatedPrincipal,
        call_id: &CallId,
        command: CallCommand,
        command_id: CommandId,
    ) -> Result<RuntimeOutput, RuntimeError> {
        let mut working_engine = self.engine.clone();
        match working_engine
            .apply_authorized_idempotent_call_command(principal, call_id, command, command_id)
        {
            Ok(output) => self.commit_engine_output(working_engine, output),
            Err(error) => {
                self.engine = working_engine;
                Err(error.into())
            }
        }
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
        self.commit_engine_output(working_engine, output)
    }

    /// Sends an inbound INVITE response after verifying the principal's
    /// command permission.
    pub fn respond_to_invite_authorized(
        &mut self,
        principal: &AuthenticatedPrincipal,
        call_id: &CallId,
        status_code: u16,
        reason: impl Into<String>,
        body: Vec<u8>,
        now: Duration,
    ) -> Result<RuntimeOutput, RuntimeError> {
        let mut working_engine = self.engine.clone();
        let result = working_engine.respond_to_invite_authorized(
            principal,
            call_id,
            status_code,
            reason,
            body,
            now,
        );
        match result {
            Ok(output) => self.commit_engine_output(working_engine, output),
            Err(error) => {
                self.engine = working_engine;
                Err(error.into())
            }
        }
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
        self.commit_runtime_output(working_engine, output)
    }

    /// Receives one transport read and applies explicit Digest credentials to
    /// any outbound INVITE 401/407 challenge in that read.
    ///
    /// Other requests and responses follow the ordinary dispatch path. The
    /// credentials and qop inputs are borrowed only for this operation and are
    /// never retained by the runtime or engine.
    ///
    /// # Errors
    ///
    /// Returns an error when transport I/O, source policy, SIP validation,
    /// Digest construction, engine processing, or action delivery fails. The
    /// engine remains unchanged on every error.
    pub fn receive_once_with_digest_auth(
        &mut self,
        now: Duration,
        credentials: &DigestCredentials,
        cnonce: Option<&str>,
        nonce_count: Option<u32>,
    ) -> Result<RuntimeOutput, RuntimeError> {
        self.receive_once_with_digest_handler(now, |engine, response| {
            Ok(engine.receive_digest_challenge(response, now, credentials, cnonce, nonce_count)?)
        })
    }

    /// Receives one transport read and resolves provider Digest credentials
    /// from the supplied authentication policy only when a new authenticated
    /// INVITE retry is required.
    ///
    /// The resolver is invoked again for each new challenge, allowing runtime
    /// credential rotation. It is not called for ordinary messages, duplicate
    /// challenge ACK replay, or a challenge rejected before authentication.
    /// Neither the credential reference nor returned credentials are retained.
    ///
    /// # Errors
    ///
    /// Returns an error when the provider does not configure Digest, the
    /// current credentials are unavailable, or ordinary transport/engine
    /// processing fails. Engine state remains unchanged on every error.
    pub fn receive_once_with_provider_digest_auth<R>(
        &mut self,
        now: Duration,
        authentication: &AuthenticationPolicy,
        resolver: &mut R,
        cnonce: Option<&str>,
        nonce_count: Option<u32>,
    ) -> Result<RuntimeOutput, RuntimeError>
    where
        R: DigestCredentialResolver + ?Sized,
    {
        self.receive_once_with_digest_handler(now, |engine, response| {
            Ok(engine.receive_digest_challenge_resolved(
                response,
                now,
                cnonce,
                nonce_count,
                || match authentication {
                    AuthenticationPolicy::None => {
                        Err(EngineError::DigestAuthenticationNotConfigured)
                    }
                    AuthenticationPolicy::Digest { credential_ref, .. } => resolver
                        .resolve(credential_ref)
                        .ok_or(EngineError::DigestCredentialsUnavailable),
                },
            )?)
        })
    }

    /// Receives one transport read and resolves Digest credentials from the
    /// authentication policy captured when a routed Rust call was originated.
    ///
    /// A routed call's provider policy is retained only by opaque SIP
    /// Call-ID-to-call context while that call is active. The context is
    /// removed when the engine emits a terminal lifecycle event. Duplicate
    /// challenge ACK replay still bypasses the resolver.
    ///
    /// # Errors
    ///
    /// Returns an error when no routed provider context exists for a 401/407
    /// response, credentials are unavailable, or ordinary transport/engine
    /// processing fails. Engine and context state remain unchanged on error.
    pub fn receive_once_with_routed_provider_digest_auth<R>(
        &mut self,
        now: Duration,
        resolver: &mut R,
        cnonce: Option<&str>,
        nonce_count: Option<u32>,
    ) -> Result<RuntimeOutput, RuntimeError>
    where
        R: DigestCredentialResolver + ?Sized,
    {
        let contexts = self.provider_authentication.clone();
        self.receive_once_with_digest_handler(now, |engine, response| {
            let sip_call_id = response
                .headers
                .get("Call-ID")
                .ok_or(EngineError::DigestAuthenticationNotConfigured)?;
            let (_, authentication) = contexts
                .get(sip_call_id)
                .ok_or(EngineError::DigestAuthenticationNotConfigured)?;
            Ok(engine.receive_digest_challenge_resolved(
                response,
                now,
                cnonce,
                nonce_count,
                || match authentication {
                    AuthenticationPolicy::None => {
                        Err(EngineError::DigestAuthenticationNotConfigured)
                    }
                    AuthenticationPolicy::Digest { credential_ref, .. } => resolver
                        .resolve(credential_ref)
                        .ok_or(EngineError::DigestCredentialsUnavailable),
                },
            )?)
        })
    }

    fn receive_once_with_digest_handler<F>(
        &mut self,
        now: Duration,
        mut handle_challenge: F,
    ) -> Result<RuntimeOutput, RuntimeError>
    where
        F: FnMut(&mut CallEngine, SipResponse) -> Result<EngineOutput, RuntimeError>,
    {
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
                SipMessage::Response(response) if matches!(response.status_code, 401 | 407) => {
                    handle_challenge(&mut working_engine, response)?
                }
                SipMessage::Response(response) => working_engine.receive_response(response, now)?,
            };
            output.append(engine_output);
        }
        self.commit_runtime_output(working_engine, output)
    }

    /// Fails active calls after the connected transport or process boundary
    /// becomes unusable, then synchronizes bridge and provider context state.
    ///
    /// The call records remain retained in `Ended` state for explicit
    /// post-call inspection and reclamation. No SIP action is emitted because
    /// the transport is assumed to be unavailable. Callers should invoke this
    /// after handling [`RuntimeError::ConnectionClosed`] or another fatal
    /// transport error before dropping or replacing the runtime.
    pub fn handle_transport_failure(&mut self) -> Result<RuntimeOutput, RuntimeError> {
        let mut working_engine = self.engine.clone();
        let mut output = RuntimeOutput::default();
        output.append(working_engine.fail_active_calls()?);
        self.commit_runtime_output(working_engine, output)
    }

    /// Prepares this runtime for a controlled process replacement.
    ///
    /// New call admission is stopped, active calls are terminalized without
    /// attempting unavailable wire actions, provider-auth contexts are
    /// removed, and every retained bridge is ended. The returned bounded
    /// report is the supervisor's handoff payload; persist or deliver it before
    /// dropping this runtime. The runtime remains draining until [`Self::resume`]
    /// is called, which is useful when a restart is cancelled.
    ///
    /// The operation is transactional. A bridge/event-bound failure leaves the
    /// runtime, engine, bridge registry, and provider contexts unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when call terminalization, bridge event
    /// synchronization, or bridge cleanup cannot be completed atomically.
    pub fn prepare_restart_handoff(&mut self) -> Result<RuntimeRestartHandoff, RuntimeError> {
        let mut working_engine = self.engine.clone();
        let engine_handoff = working_engine.prepare_restart_handoff()?;
        let mut working_bridges = self.bridges.clone();
        let mut bridge_events = Vec::new();
        if let Some(bridges) = working_bridges.as_mut() {
            bridge_events.extend(bridges.drain_events(usize::MAX)?);
            for event in engine_handoff.events() {
                synchronize_bridge_lifecycle(bridges, std::slice::from_ref(event))?;
                bridge_events.extend(bridges.drain_events(usize::MAX)?);
            }
            end_all_bridges(bridges, &mut bridge_events)?;
        }
        let mut working_provider_authentication = self.provider_authentication.clone();
        // A replacement process cannot safely resume any in-flight Digest
        // exchange. Clear every retained context, including stale entries
        // that may not have emitted a terminal lifecycle event in this pass.
        working_provider_authentication.clear();
        self.engine = working_engine;
        self.bridges = working_bridges;
        self.provider_authentication = working_provider_authentication;
        Ok(RuntimeRestartHandoff {
            engine: engine_handoff,
            bridge_events,
        })
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
        self.commit_runtime_output(working_engine, output)
    }

    fn deliver(&mut self, output: &RuntimeOutput) -> Result<(), RuntimeError> {
        for action in &output.actions {
            self.transport.send(action)?;
        }
        Ok(())
    }

    fn commit_engine_output(
        &mut self,
        working_engine: CallEngine,
        output: EngineOutput,
    ) -> Result<RuntimeOutput, RuntimeError> {
        let mut runtime_output = RuntimeOutput::default();
        runtime_output.append(output);
        self.commit_runtime_output(working_engine, runtime_output)
    }

    fn commit_runtime_output(
        &mut self,
        working_engine: CallEngine,
        mut output: RuntimeOutput,
    ) -> Result<RuntimeOutput, RuntimeError> {
        let mut working_bridges = self.bridges.clone();
        let mut working_provider_authentication = self.provider_authentication.clone();
        if let Some(bridges) = working_bridges.as_mut() {
            synchronize_bridge_lifecycle(bridges, &output.events)?;
            output
                .bridge_events
                .extend(bridges.drain_events(usize::MAX)?);
        }
        synchronize_provider_authentication(&mut working_provider_authentication, &output.events);
        self.deliver(&output)?;
        self.engine = working_engine;
        self.bridges = working_bridges;
        self.provider_authentication = working_provider_authentication;
        Ok(output)
    }
}

fn synchronize_bridge_lifecycle(
    bridges: &mut BridgeRegistry,
    events: &[LifecycleEvent],
) -> Result<(), BridgeError> {
    for event in events {
        let Some((bridge_id, state)) = human_bridge_for_call(bridges, &event.call_id)? else {
            continue;
        };
        match (event.kind, state) {
            (CallEventKind::Answered, BridgeState::ConnectingHuman) => {
                let _ = bridges.complete_human(&bridge_id)?;
            }
            (
                CallEventKind::Failed | CallEventKind::Hangup,
                BridgeState::ConnectingHuman | BridgeState::HumanActive,
            ) => {
                let _ = bridges.fail_human(&bridge_id)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn human_bridge_for_call(
    bridges: &BridgeRegistry,
    call_id: &CallId,
) -> Result<Option<(BridgeId, BridgeState)>, BridgeError> {
    Ok(bridges
        .list(bridges.config().max_bridges)?
        .into_iter()
        .find(|bridge| {
            bridge
                .pending_human
                .as_ref()
                .or(bridge.active_human.as_ref())
                .is_some_and(|human| &human.call_id == call_id)
        })
        .map(|bridge| (bridge.id, bridge.state)))
}

fn synchronize_provider_authentication(
    contexts: &mut HashMap<String, (CallId, AuthenticationPolicy)>,
    events: &[LifecycleEvent],
) {
    for event in events {
        if matches!(event.kind, CallEventKind::Failed | CallEventKind::Hangup) {
            contexts.retain(|_, (call_id, _)| call_id != &event.call_id);
        }
    }
}

fn end_all_bridges(
    bridges: &mut BridgeRegistry,
    events: &mut Vec<BridgeEvent>,
) -> Result<(), BridgeError> {
    let snapshots = bridges.list(bridges.config().max_bridges)?;
    for snapshot in snapshots {
        if snapshot.state == BridgeState::Ended {
            continue;
        }
        let _ = bridges.end(&snapshot.id)?;
        events.extend(bridges.drain_events(usize::MAX)?);
    }
    Ok(())
}

fn provider_transport_matches(
    transport: &RuntimeTransport,
    signaling_transport: SignalingTransport,
) -> bool {
    matches!(
        (transport, signaling_transport),
        (RuntimeTransport::Udp(_), SignalingTransport::Udp)
            | (RuntimeTransport::Tcp { .. }, SignalingTransport::Tcp)
            | (RuntimeTransport::Tls { .. }, SignalingTransport::Tls)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use call_api::{AuditOperation, AuditOutcome, ControlPermission};
    use call_bridge::{BridgeEventKind, BridgeRegistryConfig};
    use call_core::{CallEventKind, CallState, CommandId, StreamId};
    use call_engine::EngineConfig;
    use provider_routing::{ProviderProfile, RouteController, RoutingConfig};
    use rcgen::generate_simple_self_signed;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    use sip_auth::{DigestAlgorithm, DigestAuthorization, DigestChallenge};
    use sip_transport::{TcpTransport, TlsTransport, UdpTransport};
    use sip_types::{Headers, SipMethod, SipRequest, SipResponse};
    use std::net::{TcpListener, TcpStream};
    use std::thread;
    use std::{
        io::{Read, Write},
        sync::Arc,
    };

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
        inbound_invite_with("runtime-inbound", "runtime-inbound@example.com")
    }

    fn inbound_invite_with(branch: &str, call_id: &str) -> SipRequest {
        let mut headers = Headers::new();
        headers.push("Via", format!("SIP/2.0/UDP client.invalid;branch={branch}"));
        headers.push("From", "Alice <sip:alice@example.com>;tag=client-2");
        headers.push("To", "Bob <sip:bob@example.com>");
        headers.push("Call-ID", call_id);
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

    fn response_for_invite(request: &SipRequest, status_code: u16) -> SipMessage {
        let mut headers = Headers::new();
        headers.push("Via", request.headers.get("Via").unwrap());
        headers.push("From", request.headers.get("From").unwrap());
        headers.push("To", "Bob <sip:bob@example.com>;tag=human-peer");
        headers.push("Call-ID", request.headers.get("Call-ID").unwrap());
        headers.push("CSeq", request.headers.get("CSeq").unwrap());
        headers.push("Contact", "<sip:bob@127.0.0.1:5070>");
        SipMessage::Response(SipResponse {
            version: "SIP/2.0".to_owned(),
            status_code,
            reason: if status_code == 200 {
                "OK"
            } else {
                "Busy Here"
            }
            .to_owned(),
            headers,
            body: Vec::new(),
        })
    }

    fn tls_configs() -> (Arc<rustls::ClientConfig>, Arc<rustls::ServerConfig>) {
        let certificate = generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
        let certificate_der = CertificateDer::from(certificate.cert.der().to_vec());
        let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            certificate.key_pair.serialize_der(),
        ));
        let mut roots = rustls::RootCertStore::empty();
        roots.add(certificate_der.clone()).unwrap();
        let client = Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        );
        let server = Arc::new(
            rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(vec![certificate_der], private_key)
                .unwrap(),
        );
        (client, server)
    }

    fn digest_challenge_for_invite_with(request: &SipRequest, challenge_value: &str) -> SipMessage {
        let mut headers = Headers::new();
        headers.push("Via", request.headers.get("Via").unwrap());
        headers.push("From", request.headers.get("From").unwrap());
        headers.push("To", "Bob <sip:bob@example.com>;tag=runtime-digest");
        headers.push("Call-ID", request.headers.get("Call-ID").unwrap());
        headers.push("CSeq", request.headers.get("CSeq").unwrap());
        headers.push("WWW-Authenticate", challenge_value);
        SipMessage::Response(SipResponse {
            version: "SIP/2.0".to_owned(),
            status_code: 401,
            reason: "Unauthorized".to_owned(),
            headers,
            body: Vec::new(),
        })
    }

    fn digest_challenge_for_invite(request: &SipRequest) -> SipMessage {
        digest_challenge_for_invite_with(
            request,
            r#"Digest realm="runtime", nonce="runtime-nonce", qop="auth""#,
        )
    }

    fn routed_digest_routes() -> ProviderRouteTable {
        let profile = ProviderProfile::new(
            "routed-digest-provider",
            "sip.provider.invalid",
            5060,
            SignalingTransport::Udp,
        )
        .unwrap()
        .with_targets(EngineTarget::Rust, EngineTarget::Asterisk)
        .with_authentication(AuthenticationPolicy::Digest {
            credential_ref: "opaque-provider-reference".to_owned(),
            algorithm: DigestAlgorithm::Md5,
        });
        let mut routes = ProviderRouteTable::new(RoutingConfig::default()).unwrap();
        routes.insert(profile).unwrap();
        routes
    }

    fn receive_ack(peer: &UdpTransport) {
        let (message, _) = peer.recv().unwrap();
        assert!(matches!(
            message,
            SipMessage::Request(SipRequest {
                method: SipMethod::Ack,
                ..
            })
        ));
    }

    fn receive_ack_and_retry(peer: &UdpTransport) -> SipRequest {
        receive_ack(peer);
        let (message, _) = peer.recv().unwrap();
        let SipMessage::Request(request) = message else {
            panic!("expected authenticated INVITE retry");
        };
        assert_eq!(request.method, SipMethod::Invite);
        request
    }

    fn assert_digest_retry(
        request: &SipRequest,
        challenge_value: &str,
        credentials: &DigestCredentials,
    ) {
        let authorization =
            DigestAuthorization::parse(request.headers.get("Authorization").unwrap()).unwrap();
        assert!(authorization.verify_against(
            &DigestChallenge::parse(challenge_value).unwrap(),
            credentials,
            "INVITE",
            &request.body,
        ));
    }

    fn complete_provider_digest_call(
        runtime: &mut CallRuntime,
        peer: &UdpTransport,
        runtime_address: SocketAddr,
        call_id: &CallId,
        retry: &SipRequest,
    ) {
        peer.send_to(&response_for_invite(retry, 200), runtime_address)
            .unwrap();
        let completed = runtime.receive_once(Duration::from_millis(5)).unwrap();
        assert!(
            completed
                .events()
                .iter()
                .any(|event| event.kind == CallEventKind::Answered)
        );
        receive_ack(peer);
        assert_eq!(
            runtime.engine().snapshot(call_id).unwrap().state,
            CallState::Answered
        );
    }

    fn reject_provider_digest_retry<R>(
        runtime: &mut CallRuntime,
        peer: &UdpTransport,
        runtime_address: SocketAddr,
        policy: &AuthenticationPolicy,
        resolver: &mut R,
        retry: &SipRequest,
    ) where
        R: DigestCredentialResolver + ?Sized,
    {
        let challenge = digest_challenge_for_invite_with(
            retry,
            r#"Digest realm="runtime", nonce="rotation-nonce-3", qop="auth", stale=true"#,
        );
        peer.send_to(&challenge, runtime_address).unwrap();
        let error = runtime
            .receive_once_with_provider_digest_auth(
                Duration::from_millis(4),
                policy,
                resolver,
                Some("rotation-cnonce-3"),
                Some(1),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            RuntimeError::Engine(EngineError::DigestRetryLimitReached)
        ));
    }

    fn replay_duplicate_provider_digest_challenge<R>(
        runtime: &mut CallRuntime,
        peer: &UdpTransport,
        runtime_address: SocketAddr,
        challenge: &SipMessage,
        resolver: &mut R,
    ) where
        R: DigestCredentialResolver + ?Sized,
    {
        peer.send_to(challenge, runtime_address).unwrap();
        let output = runtime
            .receive_once_with_provider_digest_auth(
                Duration::from_millis(3),
                &AuthenticationPolicy::None,
                resolver,
                None,
                None,
            )
            .unwrap();
        assert_eq!(output.actions().len(), 1);
        receive_ack(peer);
    }

    fn bye_for_invite(request: &SipRequest) -> SipMessage {
        let mut headers = Headers::new();
        headers.push("Via", "SIP/2.0/UDP client.invalid;branch=runtime-human-bye");
        headers.push("From", "Bob <sip:bob@example.com>;tag=human-peer");
        headers.push("To", request.headers.get("From").unwrap());
        headers.push("Call-ID", request.headers.get("Call-ID").unwrap());
        headers.push("CSeq", "2 BYE");
        SipMessage::Request(SipRequest {
            method: SipMethod::Bye,
            request_uri: request.request_uri.clone(),
            version: "SIP/2.0".to_owned(),
            headers,
            body: Vec::new(),
        })
    }

    fn principal(permissions: &[ControlPermission]) -> AuthenticatedPrincipal {
        AuthenticatedPrincipal::from_verified_claims("voice-app", permissions.iter().copied())
            .unwrap()
    }

    fn runtime_with_inbound_bridge() -> (CallRuntime, BridgeId, CallId) {
        let runtime_transport = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 2_048).unwrap();
        let runtime_address = runtime_transport.local_addr().unwrap();
        let caller = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 2_048).unwrap();
        caller
            .send_to(&SipMessage::Request(inbound_invite()), runtime_address)
            .unwrap();
        let bridges = BridgeRegistry::new(BridgeRegistryConfig {
            max_pending_events: 2,
            ..BridgeRegistryConfig::default()
        })
        .unwrap();
        let mut runtime = CallRuntime::udp(
            CallEngine::new(EngineConfig::default()).unwrap(),
            runtime_transport,
        )
        .with_bridge_registry(bridges);
        let inbound = runtime.receive_once(Duration::ZERO).unwrap();
        let caller_id = inbound.events()[0].call_id.clone();
        let (trying, _) = caller.recv().unwrap();
        assert!(matches!(
            trying,
            SipMessage::Response(SipResponse {
                status_code: 100,
                ..
            })
        ));
        let (bridge_id, _) = runtime
            .bridge_registry_mut()
            .unwrap()
            .create_ai(
                caller_id.clone(),
                LegId::from_sequence(1),
                StreamId::from_sequence(1),
            )
            .unwrap();
        (runtime, bridge_id, caller_id)
    }

    #[test]
    fn runtime_metrics_delegate_without_exposing_call_identifiers() {
        let runtime_transport = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 2_048).unwrap();
        let peer = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 2_048).unwrap();
        let mut runtime = CallRuntime::udp(
            CallEngine::new(EngineConfig::default()).unwrap(),
            runtime_transport,
        );
        let (call_id, _) = runtime
            .originate(invite(), peer.local_addr().unwrap(), Duration::ZERO)
            .unwrap();
        let _ = peer.recv().unwrap();

        let metrics = runtime.metrics();
        assert_eq!(metrics.calls.calls_started_total, 1);
        assert_eq!(metrics.calls.calls_active, 1);
        assert_eq!(metrics.active_transactions, 1);
        assert_eq!(metrics.active_dialogs, 1);
        assert!(!metrics.prometheus().contains(call_id.as_str()));
    }

    #[test]
    fn runtime_diagnostics_delegate_and_preserve_read_authorization() {
        let runtime_transport = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 2_048).unwrap();
        let peer = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 2_048).unwrap();
        let mut runtime = CallRuntime::udp(
            CallEngine::new(EngineConfig::default()).unwrap(),
            runtime_transport,
        );
        let (call_id, _) = runtime
            .originate(invite(), peer.local_addr().unwrap(), Duration::ZERO)
            .unwrap();
        let _ = peer.recv().unwrap();

        let diagnostics = runtime.diagnostics(&call_id).unwrap();
        assert_eq!(diagnostics.call_id, call_id);
        assert_eq!(diagnostics.signaling.client_transactions, 1);
        assert!(!format!("{diagnostics:?}").contains("runtime-invite@example.com"));

        let reader = principal(&[ControlPermission::ReadCalls]);
        assert_eq!(
            runtime.diagnostics_authorized(&reader, &call_id).unwrap(),
            diagnostics
        );
        assert!(matches!(
            runtime.diagnostics_authorized(&principal(&[]), &CallId::from_sequence(99)),
            Err(RuntimeError::Engine(EngineError::CallApi(
                call_api::ApiError::ReadPermissionDenied
            )))
        ));
        assert_eq!(runtime.list_diagnostics(8).unwrap(), vec![diagnostics]);
    }

    #[test]
    fn runtime_exposes_authorized_audit_records_without_credentials_or_sip_ids() {
        let runtime_transport = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 2_048).unwrap();
        let peer = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 2_048).unwrap();
        let mut runtime = CallRuntime::udp(
            CallEngine::new(EngineConfig::default()).unwrap(),
            runtime_transport,
        );
        let originate = AuthenticatedPrincipal::from_verified_claims(
            "runtime-app",
            [ControlPermission::OriginateCalls],
        )
        .unwrap();
        let reader = AuthenticatedPrincipal::from_verified_claims(
            "runtime-reader",
            [ControlPermission::ReadCalls],
        )
        .unwrap();
        assert!(matches!(
            runtime.originate_authorized(
                &reader,
                invite(),
                peer.local_addr().unwrap(),
                Duration::ZERO,
            ),
            Err(RuntimeError::Engine(EngineError::CallApi(
                call_api::ApiError::PermissionDenied {
                    command: CallCommand::InviteReceived,
                    permission: ControlPermission::OriginateCalls,
                },
            )))
        ));
        let denied = runtime.drain_audit_records(&reader, 8).unwrap();
        assert_eq!(denied.len(), 1);
        assert_eq!(denied[0].principal_id, "runtime-reader");
        assert_eq!(denied[0].call_id, None);
        assert_eq!(denied[0].operation, AuditOperation::Originate);
        assert_eq!(
            denied[0].outcome,
            AuditOutcome::Rejected("permission_denied")
        );

        let (call_id, _) = runtime
            .originate_authorized(
                &originate,
                invite(),
                peer.local_addr().unwrap(),
                Duration::ZERO,
            )
            .unwrap();
        let _ = peer.recv().unwrap();

        let records = runtime.drain_audit_records(&reader, 8).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].principal_id, "runtime-app");
        assert_eq!(records[0].call_id, Some(call_id));
        assert_eq!(records[0].operation, AuditOperation::Originate);
        assert_eq!(records[0].outcome, AuditOutcome::Succeeded);
        assert!(!format!("{records:?}").contains("runtime-invite@example.com"));
        assert!(!format!("{runtime:?}").contains("password"));
    }

    #[test]
    fn runtime_health_delegates_capacity_aware_readiness() {
        let runtime_transport = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 2_048).unwrap();
        let peer = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 2_048).unwrap();
        let mut runtime = CallRuntime::udp(
            CallEngine::new(EngineConfig {
                call_registry: call_api::CallRegistryConfig {
                    max_calls: 1,
                    ..call_api::CallRegistryConfig::default()
                },
                max_transactions: 1,
                ..EngineConfig::default()
            })
            .unwrap(),
            runtime_transport,
        );
        assert!(runtime.health().live);
        assert!(runtime.health().ready);

        let (call_id, _) = runtime
            .originate(invite(), peer.local_addr().unwrap(), Duration::ZERO)
            .unwrap();
        let _ = peer.recv().unwrap();
        let saturated = runtime.health();
        assert!(!saturated.ready);
        assert_eq!(saturated.retained_calls, 1);
        assert_eq!(saturated.active_transactions, 1);
        assert!(!saturated.prometheus().contains(call_id.as_str()));

        runtime
            .apply_call_command(&call_id, CallCommand::Answer)
            .unwrap();
        runtime
            .apply_call_command(&call_id, CallCommand::Hangup)
            .unwrap();
        runtime
            .apply_call_command(&call_id, CallCommand::End)
            .unwrap();
        assert!(!runtime.health().ready);
        runtime
            .engine_mut()
            .reclaim_terminal_call(&call_id)
            .unwrap();
        assert!(runtime.health().ready);
    }

    #[test]
    fn runtime_drain_rejects_new_calls_and_preserves_existing_dialogs() {
        let runtime_transport = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 2_048).unwrap();
        let runtime_address = runtime_transport.local_addr().unwrap();
        let peer = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 2_048).unwrap();
        let mut runtime = CallRuntime::udp(
            CallEngine::new(EngineConfig::default()).unwrap(),
            runtime_transport,
        );

        peer.send_to(&SipMessage::Request(inbound_invite()), runtime_address)
            .unwrap();
        let accepted = runtime.receive_once(Duration::ZERO).unwrap();
        let existing_call = accepted.events()[0].call_id.clone();
        assert!(matches!(
            peer.recv().unwrap().0,
            SipMessage::Response(SipResponse {
                status_code: 100,
                ..
            })
        ));

        runtime.begin_drain();
        assert!(runtime.is_draining());
        assert!(runtime.health().draining);
        assert!(!runtime.health().ready);
        assert!(
            runtime
                .health()
                .prometheus()
                .contains("engine_draining 1\n")
        );

        let outbound_error = runtime
            .originate(invite(), peer.local_addr().unwrap(), Duration::ZERO)
            .unwrap_err();
        assert!(matches!(
            outbound_error,
            RuntimeError::Engine(EngineError::Draining)
        ));
        assert_eq!(runtime.metrics().calls.calls_retained, 1);

        let new_invite = inbound_invite_with("runtime-drain-new", "runtime-drain-new@example.com");
        peer.send_to(&SipMessage::Request(new_invite), runtime_address)
            .unwrap();
        let rejected = runtime.receive_once(Duration::ZERO).unwrap();
        assert!(rejected.events().is_empty());
        assert!(matches!(
            &peer.recv().unwrap().0,
            SipMessage::Response(SipResponse {
                status_code: 503,
                reason,
                ..
            }) if reason == "Service Unavailable"
        ));
        assert_eq!(runtime.metrics().calls.calls_retained, 1);

        let continued = runtime
            .respond_to_invite(&existing_call, 200, "OK", Vec::new(), Duration::ZERO)
            .unwrap();
        assert!(matches!(
            continued.actions()[0].message,
            SipMessage::Response(SipResponse {
                status_code: 200,
                ..
            })
        ));
        assert!(matches!(
            peer.recv().unwrap().0,
            SipMessage::Response(SipResponse {
                status_code: 200,
                ..
            })
        ));

        runtime.resume();
        assert!(!runtime.is_draining());
        assert!(runtime.health().ready);
        let (resumed_call, _) = runtime
            .originate(invite(), peer.local_addr().unwrap(), Duration::ZERO)
            .unwrap();
        assert_eq!(resumed_call.as_str(), "call_2");
        assert!(matches!(
            peer.recv().unwrap().0,
            SipMessage::Request(SipRequest {
                method: SipMethod::Invite,
                ..
            })
        ));
    }

    #[test]
    fn udp_runtime_replays_idempotent_command_events_without_duplicate_state_changes() {
        let runtime_transport = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 2_048).unwrap();
        let peer = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 2_048).unwrap();
        let mut runtime = CallRuntime::udp(
            CallEngine::new(EngineConfig::default()).unwrap(),
            runtime_transport,
        );
        let (call_id, _) = runtime
            .originate(invite(), peer.local_addr().unwrap(), Duration::ZERO)
            .unwrap();
        let _ = peer.recv().unwrap();
        let command_id = CommandId::from_sequence(1);

        let first = runtime
            .apply_idempotent_call_command(&call_id, CallCommand::Hangup, command_id.clone())
            .unwrap();
        let retry = runtime
            .apply_idempotent_call_command(&call_id, CallCommand::Hangup, command_id.clone())
            .unwrap();
        assert_eq!(retry.events(), first.events());
        assert_eq!(
            runtime.engine().snapshot(&call_id).unwrap().state,
            CallState::Ending
        );

        assert!(matches!(
            runtime.apply_idempotent_call_command(&call_id, CallCommand::End, command_id),
            Err(RuntimeError::Engine(EngineError::CallApi(
                call_api::ApiError::IdempotencyConflict,
            )))
        ));
        assert_eq!(
            runtime.engine().snapshot(&call_id).unwrap().state,
            CallState::Ending
        );
    }

    #[test]
    fn udp_runtime_authorized_commands_preserve_atomic_denials() {
        let runtime_transport = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 2_048).unwrap();
        let peer = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 2_048).unwrap();
        let mut runtime = CallRuntime::udp(
            CallEngine::new(EngineConfig::default()).unwrap(),
            runtime_transport,
        );
        let (call_id, _) = runtime
            .originate(invite(), peer.local_addr().unwrap(), Duration::ZERO)
            .unwrap();
        let _ = peer.recv().unwrap();
        let read_only = principal(&[ControlPermission::ReadCalls]);
        let hangup = principal(&[ControlPermission::HangupCalls]);

        assert!(matches!(
            runtime.apply_authorized_call_command(&read_only, &call_id, CallCommand::Hangup),
            Err(RuntimeError::Engine(EngineError::CallApi(
                call_api::ApiError::PermissionDenied {
                    command: CallCommand::Hangup,
                    permission: ControlPermission::HangupCalls,
                },
            )))
        ));
        assert_eq!(
            runtime.engine().snapshot(&call_id).unwrap().state,
            CallState::Inviting
        );
        let denied = runtime.drain_audit_records(&read_only, 8).unwrap();
        assert_eq!(denied.len(), 1);
        assert_eq!(denied[0].call_id, Some(call_id.clone()));
        assert_eq!(
            denied[0].outcome,
            AuditOutcome::Rejected("permission_denied")
        );

        let command_id = CommandId::from_sequence(77);
        let first = runtime
            .apply_authorized_idempotent_call_command(
                &hangup,
                &call_id,
                CallCommand::Hangup,
                command_id.clone(),
            )
            .unwrap();
        let retry = runtime
            .apply_authorized_idempotent_call_command(
                &hangup,
                &call_id,
                CallCommand::Hangup,
                command_id,
            )
            .unwrap();
        assert_eq!(retry.events(), first.events());
        assert_eq!(
            runtime.engine().snapshot(&call_id).unwrap().state,
            CallState::Ending
        );
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
    fn provider_route_gates_rust_origination_and_resolves_address_lazily() {
        let peer = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 2_048).unwrap();
        let runtime_transport = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 2_048).unwrap();
        let mut runtime = CallRuntime::udp(
            CallEngine::new(EngineConfig::default()).unwrap(),
            runtime_transport,
        );
        let resolver_calls = std::cell::Cell::new(0);
        let mut resolver = |host: &str, port: u16| {
            resolver_calls.set(resolver_calls.get() + 1);
            assert_eq!(host, "sip.provider.invalid");
            assert_eq!(port, 5060);
            Some(peer.local_addr().unwrap())
        };

        let mut routes = ProviderRouteTable::new(RoutingConfig::default()).unwrap();
        let asterisk_profile = ProviderProfile::new(
            "asterisk-provider",
            "sip.provider.invalid",
            5060,
            SignalingTransport::Udp,
        )
        .unwrap();
        routes.insert(asterisk_profile).unwrap();
        let fallback = runtime
            .originate_with_provider_route(
                &routes,
                "asterisk-provider",
                invite(),
                &mut resolver,
                Duration::ZERO,
            )
            .unwrap_err();
        assert!(matches!(
            fallback,
            RuntimeError::ProviderRouteFallback {
                matched_by: RouteMatch::OutboundProvider
            }
        ));
        assert_eq!(resolver_calls.get(), 0);
        assert!(runtime.engine().list(10).unwrap().is_empty());

        let mut rust_routes = ProviderRouteTable::new(RoutingConfig::default()).unwrap();
        let rust_profile = ProviderProfile::new(
            "rust-provider",
            "sip.provider.invalid",
            5060,
            SignalingTransport::Udp,
        )
        .unwrap()
        .with_targets(EngineTarget::Rust, EngineTarget::Asterisk);
        rust_routes.insert(rust_profile).unwrap();
        let (call_id, output) = runtime
            .originate_with_provider_route(
                &rust_routes,
                "rust-provider",
                invite(),
                &mut resolver,
                Duration::ZERO,
            )
            .unwrap();
        assert_eq!(resolver_calls.get(), 1);
        assert_eq!(output.actions().len(), 1);
        assert_eq!(
            runtime.engine().snapshot(&call_id).unwrap().state,
            CallState::Inviting
        );
        let (message, _) = peer.recv().unwrap();
        assert!(matches!(
            message,
            SipMessage::Request(SipRequest {
                method: SipMethod::Invite,
                ..
            })
        ));
    }

    #[test]
    fn route_controller_rollback_blocks_runtime_resolution_without_mutation() {
        let peer = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 2_048).unwrap();
        let runtime_transport = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 2_048).unwrap();
        let mut runtime = CallRuntime::udp(
            CallEngine::new(EngineConfig::default()).unwrap(),
            runtime_transport,
        );
        let mut routes = RouteController::new(routed_digest_routes());
        let resolver_calls = std::cell::Cell::new(0);
        let mut resolver = |host: &str, port: u16| {
            resolver_calls.set(resolver_calls.get() + 1);
            assert_eq!(host, "sip.provider.invalid");
            assert_eq!(port, 5060);
            Some(peer.local_addr().unwrap())
        };

        let blocked = runtime
            .originate_with_route_controller(
                &routes,
                "routed-digest-provider",
                invite(),
                &mut resolver,
                Duration::ZERO,
            )
            .unwrap_err();
        assert!(matches!(
            blocked,
            RuntimeError::ProviderRouteFallback {
                matched_by: RouteMatch::OutboundProvider
            }
        ));
        assert_eq!(resolver_calls.get(), 0);
        assert!(runtime.engine().list(10).unwrap().is_empty());

        routes.activate_rust(routes.state().generation()).unwrap();
        let (call_id, output) = runtime
            .originate_with_route_controller(
                &routes,
                "routed-digest-provider",
                invite(),
                &mut resolver,
                Duration::ZERO,
            )
            .unwrap();
        assert_eq!(resolver_calls.get(), 1);
        assert_eq!(output.actions().len(), 1);
        assert_eq!(
            runtime.engine().snapshot(&call_id).unwrap().state,
            CallState::Inviting
        );
        let _ = peer.recv().unwrap();

        routes
            .rollback_to_asterisk(routes.state().generation())
            .unwrap();
        let blocked_again = runtime
            .originate_with_route_controller(
                &routes,
                "routed-digest-provider",
                invite(),
                &mut resolver,
                Duration::ZERO,
            )
            .unwrap_err();
        assert!(matches!(
            blocked_again,
            RuntimeError::ProviderRouteFallback {
                matched_by: RouteMatch::OutboundProvider
            }
        ));
        assert_eq!(resolver_calls.get(), 1);
        assert_eq!(runtime.engine().list(10).unwrap().len(), 1);
    }

    #[test]
    fn provider_route_rejects_unsupported_or_unresolved_destinations_atomically() {
        let mut routes = ProviderRouteTable::new(RoutingConfig::default()).unwrap();
        let tls_profile = ProviderProfile::new(
            "tls-provider",
            "tls.provider.invalid",
            5061,
            SignalingTransport::Tls,
        )
        .unwrap()
        .with_targets(EngineTarget::Rust, EngineTarget::Asterisk);
        routes.insert(tls_profile).unwrap();

        let runtime_transport = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 2_048).unwrap();
        let mut runtime = CallRuntime::udp(
            CallEngine::new(EngineConfig::default()).unwrap(),
            runtime_transport,
        );
        let resolver_calls = std::cell::Cell::new(0);
        let mut resolver = |host: &str, _: u16| {
            resolver_calls.set(resolver_calls.get() + 1);
            assert_eq!(host, "udp.provider.invalid");
            None
        };
        let unsupported = runtime
            .originate_with_provider_route(
                &routes,
                "tls-provider",
                invite(),
                &mut resolver,
                Duration::ZERO,
            )
            .unwrap_err();
        assert!(matches!(
            unsupported,
            RuntimeError::ProviderTransportUnsupported
        ));
        assert_eq!(resolver_calls.get(), 0);
        assert!(runtime.engine().list(10).unwrap().is_empty());

        let udp_profile = ProviderProfile::new(
            "udp-provider",
            "udp.provider.invalid",
            5060,
            SignalingTransport::Udp,
        )
        .unwrap()
        .with_targets(EngineTarget::Rust, EngineTarget::Asterisk);
        routes.insert(udp_profile).unwrap();
        let unresolved = runtime
            .originate_with_provider_route(
                &routes,
                "udp-provider",
                invite(),
                &mut resolver,
                Duration::ZERO,
            )
            .unwrap_err();
        assert!(matches!(
            unresolved,
            RuntimeError::ProviderAddressUnavailable
        ));
        assert_eq!(resolver_calls.get(), 1);
        assert!(runtime.engine().list(10).unwrap().is_empty());
    }

    #[test]
    fn routed_provider_digest_context_resolves_and_duplicate_replays_without_resolver() {
        let peer = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 4_096).unwrap();
        let runtime_transport = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 4_096).unwrap();
        let runtime_address = runtime_transport.local_addr().unwrap();
        let mut runtime = CallRuntime::udp(
            CallEngine::new(EngineConfig::default()).unwrap(),
            runtime_transport,
        );
        let routes = routed_digest_routes();
        let mut address_resolver = |host: &str, port: u16| {
            assert_eq!(host, "sip.provider.invalid");
            assert_eq!(port, 5060);
            Some(peer.local_addr().unwrap())
        };
        let (call_id, _) = runtime
            .originate_with_provider_route(
                &routes,
                "routed-digest-provider",
                invite(),
                &mut address_resolver,
                Duration::ZERO,
            )
            .unwrap();
        let (originated, _) = peer.recv().unwrap();
        let SipMessage::Request(originated) = originated else {
            panic!("expected originated INVITE");
        };
        assert_eq!(
            runtime
                .provider_authentication
                .get("runtime-invite@example.com")
                .map(|(stored_call_id, _)| stored_call_id),
            Some(&call_id)
        );

        let credentials = DigestCredentials::new("runtime-user", "routed-runtime-secret");
        let resolutions = std::cell::Cell::new(0);
        let mut resolver = |credential_ref: &str| {
            resolutions.set(resolutions.get() + 1);
            assert_eq!(credential_ref, "opaque-provider-reference");
            Some(credentials.clone())
        };
        let challenge = digest_challenge_for_invite(&originated);
        peer.send_to(&challenge, runtime_address).unwrap();
        let challenged = runtime
            .receive_once_with_routed_provider_digest_auth(
                Duration::from_millis(1),
                &mut resolver,
                Some("routed-cnonce"),
                Some(1),
            )
            .unwrap();
        assert_eq!(challenged.actions().len(), 2);
        assert_eq!(resolutions.get(), 1);
        let retry = receive_ack_and_retry(&peer);
        assert_digest_retry(
            &retry,
            r#"Digest realm="runtime", nonce="runtime-nonce", qop="auth""#,
            &credentials,
        );
        assert_eq!(
            runtime.engine().snapshot(&call_id).unwrap().state,
            CallState::Inviting
        );

        peer.send_to(&challenge, runtime_address).unwrap();
        let duplicate = runtime
            .receive_once_with_routed_provider_digest_auth(
                Duration::from_millis(2),
                &mut resolver,
                None,
                None,
            )
            .unwrap();
        assert_eq!(duplicate.actions().len(), 1);
        assert_eq!(resolutions.get(), 1);
        receive_ack(&peer);

        assert!(!format!("{runtime:?}").contains("routed-runtime-secret"));
        assert!(!format!("{runtime:?}").contains("opaque-provider-reference"));
    }

    #[test]
    fn routed_provider_digest_without_context_is_atomic_and_does_not_resolve() {
        let peer = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 4_096).unwrap();
        let runtime_transport = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 4_096).unwrap();
        let runtime_address = runtime_transport.local_addr().unwrap();
        let mut runtime = CallRuntime::udp(
            CallEngine::new(EngineConfig::default()).unwrap(),
            runtime_transport,
        );
        let (call_id, _) = runtime
            .originate(invite(), peer.local_addr().unwrap(), Duration::ZERO)
            .unwrap();
        let (originated, _) = peer.recv().unwrap();
        let SipMessage::Request(originated) = originated else {
            panic!("expected originated INVITE");
        };
        let transaction_count = runtime.engine().transaction_count();
        let resolutions = std::cell::Cell::new(0);
        let mut resolver = |_: &str| {
            resolutions.set(resolutions.get() + 1);
            Some(DigestCredentials::new("runtime-user", "unexpected-secret"))
        };

        peer.send_to(&digest_challenge_for_invite(&originated), runtime_address)
            .unwrap();
        let error = runtime
            .receive_once_with_routed_provider_digest_auth(
                Duration::from_millis(1),
                &mut resolver,
                Some("missing-context-cnonce"),
                Some(1),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            RuntimeError::Engine(EngineError::DigestAuthenticationNotConfigured)
        ));
        assert_eq!(resolutions.get(), 0);
        assert_eq!(runtime.engine().transaction_count(), transaction_count);
        assert_eq!(
            runtime.engine().snapshot(&call_id).unwrap().state,
            CallState::Inviting
        );
        assert!(runtime.provider_authentication.is_empty());
        assert!(!format!("{error:?}").contains("unexpected-secret"));
    }

    #[test]
    fn routed_provider_digest_context_is_removed_after_failed_call() {
        let peer = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 4_096).unwrap();
        let runtime_transport = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 4_096).unwrap();
        let runtime_address = runtime_transport.local_addr().unwrap();
        let mut runtime = CallRuntime::udp(
            CallEngine::new(EngineConfig::default()).unwrap(),
            runtime_transport,
        );
        let routes = routed_digest_routes();
        let mut address_resolver = |_: &str, _: u16| Some(peer.local_addr().unwrap());
        let (call_id, _) = runtime
            .originate_with_provider_route(
                &routes,
                "routed-digest-provider",
                invite(),
                &mut address_resolver,
                Duration::ZERO,
            )
            .unwrap();
        let (originated, _) = peer.recv().unwrap();
        let SipMessage::Request(originated) = originated else {
            panic!("expected originated INVITE");
        };
        let credentials = DigestCredentials::new("runtime-user", "failed-call-secret");
        let mut resolver = |_: &str| Some(credentials.clone());
        peer.send_to(&digest_challenge_for_invite(&originated), runtime_address)
            .unwrap();
        runtime
            .receive_once_with_routed_provider_digest_auth(
                Duration::from_millis(1),
                &mut resolver,
                Some("failed-cnonce"),
                Some(1),
            )
            .unwrap();
        let retry = receive_ack_and_retry(&peer);

        peer.send_to(&response_for_invite(&retry, 486), runtime_address)
            .unwrap();
        let failed = runtime.receive_once(Duration::from_millis(2)).unwrap();
        assert!(
            failed
                .events()
                .iter()
                .any(|event| { event.call_id == call_id && event.kind == CallEventKind::Failed })
        );
        assert_eq!(
            runtime.engine().snapshot(&call_id).unwrap().state,
            CallState::Ended
        );
        assert!(runtime.provider_authentication.is_empty());
        let (ack, _) = peer.recv().unwrap();
        assert!(matches!(
            ack,
            SipMessage::Request(SipRequest {
                method: SipMethod::Ack,
                ..
            })
        ));

        let resolutions = std::cell::Cell::new(0);
        let mut post_terminal_resolver = |_: &str| {
            resolutions.set(resolutions.get() + 1);
            Some(DigestCredentials::new(
                "runtime-user",
                "post-terminal-secret",
            ))
        };
        peer.send_to(
            &digest_challenge_for_invite_with(
                &retry,
                r#"Digest realm="runtime", nonce="post-terminal-nonce", qop="auth""#,
            ),
            runtime_address,
        )
        .unwrap();
        let error = runtime
            .receive_once_with_routed_provider_digest_auth(
                Duration::from_millis(3),
                &mut post_terminal_resolver,
                Some("post-terminal-cnonce"),
                Some(1),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            RuntimeError::Engine(EngineError::DigestAuthenticationNotConfigured)
        ));
        assert_eq!(resolutions.get(), 0);
    }

    #[test]
    fn routed_provider_digest_context_is_removed_after_remote_hangup() {
        let peer = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 4_096).unwrap();
        let runtime_transport = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 4_096).unwrap();
        let runtime_address = runtime_transport.local_addr().unwrap();
        let mut runtime = CallRuntime::udp(
            CallEngine::new(EngineConfig::default()).unwrap(),
            runtime_transport,
        );
        let routes = routed_digest_routes();
        let mut address_resolver = |_: &str, _: u16| Some(peer.local_addr().unwrap());
        let (call_id, _) = runtime
            .originate_with_provider_route(
                &routes,
                "routed-digest-provider",
                invite(),
                &mut address_resolver,
                Duration::ZERO,
            )
            .unwrap();
        let (originated, _) = peer.recv().unwrap();
        let SipMessage::Request(originated) = originated else {
            panic!("expected originated INVITE");
        };
        let credentials = DigestCredentials::new("runtime-user", "hangup-secret");
        let mut resolver = |_: &str| Some(credentials.clone());
        peer.send_to(&digest_challenge_for_invite(&originated), runtime_address)
            .unwrap();
        runtime
            .receive_once_with_routed_provider_digest_auth(
                Duration::from_millis(1),
                &mut resolver,
                Some("hangup-cnonce"),
                Some(1),
            )
            .unwrap();
        let retry = receive_ack_and_retry(&peer);
        complete_provider_digest_call(&mut runtime, &peer, runtime_address, &call_id, &retry);
        assert!(
            runtime
                .provider_authentication
                .contains_key("runtime-invite@example.com")
        );

        peer.send_to(&bye_for_invite(&retry), runtime_address)
            .unwrap();
        let hung_up = runtime.receive_once(Duration::from_millis(3)).unwrap();
        assert!(
            hung_up
                .events()
                .iter()
                .any(|event| { event.call_id == call_id && event.kind == CallEventKind::Hangup })
        );
        assert_eq!(
            runtime.engine().snapshot(&call_id).unwrap().state,
            CallState::Ended
        );
        assert!(runtime.provider_authentication.is_empty());
        let (response, _) = peer.recv().unwrap();
        assert!(matches!(
            response,
            SipMessage::Response(SipResponse {
                status_code: 200,
                ..
            })
        ));
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
    fn tcp_connection_loss_emits_terminal_events_and_releases_call_state() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (close_tx, close_rx) = std::sync::mpsc::channel();
        let client_thread = thread::spawn(move || {
            let mut stream = TcpStream::connect(address).unwrap();
            ready_tx.send(()).unwrap();
            let mut invite_wire = Vec::new();
            let mut chunk = [0; 256];
            loop {
                let bytes_read = stream.read(&mut chunk).unwrap();
                if bytes_read == 0 {
                    break;
                }
                invite_wire.extend_from_slice(&chunk[..bytes_read]);
                if invite_wire.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            close_rx.recv().unwrap();
            drop(stream);
        });

        let (stream, peer) = listener.accept().unwrap();
        ready_rx.recv().unwrap();
        let transport = TcpTransport::from_stream(stream, 2_048).unwrap();
        let mut runtime = CallRuntime::tcp(
            CallEngine::new(EngineConfig::default()).unwrap(),
            transport,
            peer,
        );
        let (call_id, originated) = runtime.originate(invite(), peer, Duration::ZERO).unwrap();
        assert_eq!(originated.actions().len(), 1);
        assert_eq!(runtime.metrics().active_transactions, 1);
        assert_eq!(runtime.metrics().active_dialogs, 1);

        close_tx.send(()).unwrap();
        client_thread.join().unwrap();
        let error = runtime.receive_once(Duration::ZERO).unwrap_err();
        assert!(matches!(error, RuntimeError::ConnectionClosed { peer: actual } if actual == peer));

        let cleanup = runtime.handle_transport_failure().unwrap();
        assert!(cleanup.actions().is_empty());
        assert_eq!(
            cleanup
                .events()
                .iter()
                .map(|event| event.kind)
                .collect::<Vec<_>>(),
            vec![CallEventKind::Failed, CallEventKind::Ended]
        );
        assert_eq!(
            runtime.engine().snapshot(&call_id).unwrap().state,
            CallState::Ended
        );
        assert_eq!(runtime.metrics().calls.calls_active, 0);
        assert_eq!(runtime.metrics().active_transactions, 0);
        assert_eq!(runtime.metrics().active_dialogs, 0);
        assert_eq!(runtime.metrics().calls.calls_retained, 1);

        runtime
            .engine_mut()
            .reclaim_terminal_call(&call_id)
            .unwrap();
        assert_eq!(runtime.metrics().calls.calls_retained, 0);
    }

    #[test]
    fn restart_handoff_ends_bridges_and_reclaims_runtime_contexts() {
        let (mut runtime, bridge_id, caller_id) = runtime_with_inbound_bridge();
        let (human_call, originated) = runtime
            .originate_human_leg(
                &bridge_id,
                LegId::from_sequence(2),
                invite(),
                "127.0.0.1:5090".parse().unwrap(),
                Duration::ZERO,
            )
            .unwrap();
        assert_eq!(originated.actions().len(), 1);
        assert_eq!(
            runtime
                .bridge_registry()
                .unwrap()
                .snapshot(&bridge_id)
                .unwrap()
                .state,
            BridgeState::ConnectingHuman
        );
        runtime.provider_authentication.insert(
            "restart-handoff-sip-call".to_owned(),
            (human_call.clone(), AuthenticationPolicy::None),
        );
        assert_eq!(runtime.provider_authentication.len(), 1);

        let handoff = runtime.prepare_restart_handoff().unwrap();
        assert_eq!(
            handoff.terminated_calls(),
            &[caller_id.clone(), human_call.clone()]
        );
        assert_eq!(
            handoff
                .events()
                .iter()
                .map(|event| (event.call_id.clone(), event.kind))
                .collect::<Vec<_>>(),
            vec![
                (caller_id.clone(), CallEventKind::Failed),
                (caller_id.clone(), CallEventKind::Ended),
                (human_call.clone(), CallEventKind::Failed),
                (human_call.clone(), CallEventKind::Ended),
            ]
        );
        assert_eq!(
            handoff
                .bridge_events()
                .iter()
                .map(|event| event.kind)
                .collect::<Vec<_>>(),
            vec![BridgeEventKind::HumanFailed, BridgeEventKind::Ended]
        );
        assert!(handoff.health().draining);
        assert_eq!(
            runtime
                .bridge_registry()
                .unwrap()
                .snapshot(&bridge_id)
                .unwrap()
                .state,
            BridgeState::Ended
        );
        assert_eq!(runtime.metrics().calls.calls_active, 0);
        assert_eq!(runtime.metrics().active_transactions, 0);
        assert_eq!(runtime.metrics().active_dialogs, 0);
        assert!(runtime.provider_authentication.is_empty());

        runtime.provider_authentication.insert(
            "stale-after-handoff".to_owned(),
            (caller_id.clone(), AuthenticationPolicy::None),
        );
        let repeated = runtime.prepare_restart_handoff().unwrap();
        assert!(repeated.is_empty());
        assert!(repeated.events().is_empty());
        assert!(repeated.bridge_events().is_empty());
        assert_eq!(repeated.health(), handoff.health());
        assert!(runtime.provider_authentication.is_empty());

        runtime.resume();
        assert!(runtime.health().ready);
        runtime
            .engine_mut()
            .reclaim_terminal_call(&caller_id)
            .unwrap();
        runtime
            .engine_mut()
            .reclaim_terminal_call(&human_call)
            .unwrap();
        runtime
            .bridge_registry_mut()
            .unwrap()
            .remove_terminal(&bridge_id)
            .unwrap();
        assert_eq!(runtime.metrics().calls.calls_retained, 0);
    }

    #[test]
    fn tls_runtime_dispatches_options_and_writes_framed_response() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (client_config, server_config) = tls_configs();
        let client_thread = thread::spawn(move || {
            let mut transport =
                TlsTransport::connect(address, "localhost", client_config, 2_048).unwrap();
            transport.send(&options()).unwrap();
            loop {
                let messages = transport.recv().unwrap();
                if !messages.is_empty() {
                    return messages;
                }
            }
        });
        let (stream, peer) = listener.accept().unwrap();
        let transport = TlsTransport::accept(stream, server_config, 2_048).unwrap();
        let mut runtime = CallRuntime::tls(
            CallEngine::new(EngineConfig::default()).unwrap(),
            transport,
            peer,
        );
        let output = runtime.receive_once(Duration::ZERO).unwrap();
        assert_eq!(output.actions().len(), 1);
        let response = client_thread.join().unwrap();
        assert!(matches!(
            response.as_slice(),
            [SipMessage::Response(SipResponse {
                status_code: 200,
                ..
            })]
        ));
    }

    #[test]
    fn tls_provider_route_matches_connected_tls_runtime() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (client_config, server_config) = tls_configs();
        let provider_thread = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut transport = TlsTransport::accept(stream, server_config, 2_048).unwrap();
            loop {
                let messages = transport.recv().unwrap();
                if !messages.is_empty() {
                    return messages;
                }
            }
        });
        let transport = TlsTransport::connect(address, "localhost", client_config, 2_048).unwrap();
        let mut runtime = CallRuntime::tls(
            CallEngine::new(EngineConfig::default()).unwrap(),
            transport,
            address,
        );
        let mut routes = ProviderRouteTable::new(RoutingConfig::default()).unwrap();
        routes
            .insert(
                ProviderProfile::new(
                    "tls-provider",
                    "tls.provider.invalid",
                    address.port(),
                    SignalingTransport::Tls,
                )
                .unwrap()
                .with_targets(EngineTarget::Rust, EngineTarget::Asterisk),
            )
            .unwrap();
        let resolver_calls = std::cell::Cell::new(0);
        let mut resolver = |host: &str, port: u16| {
            resolver_calls.set(resolver_calls.get() + 1);
            assert_eq!(host, "tls.provider.invalid");
            assert_eq!(port, address.port());
            Some(address)
        };
        let (call_id, output) = runtime
            .originate_with_provider_route(
                &routes,
                "tls-provider",
                invite(),
                &mut resolver,
                Duration::ZERO,
            )
            .unwrap();
        assert_eq!(resolver_calls.get(), 1);
        assert_eq!(output.actions().len(), 1);
        assert_eq!(
            runtime.engine().snapshot(&call_id).unwrap().state,
            CallState::Inviting
        );
        let request = provider_thread.join().unwrap();
        assert!(matches!(
            request.as_slice(),
            [SipMessage::Request(SipRequest {
                method: SipMethod::Invite,
                ..
            })]
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

    #[test]
    fn udp_runtime_delivers_digest_ack_retry_and_authenticated_completion() {
        let peer = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 4_096).unwrap();
        let runtime_transport = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 4_096).unwrap();
        let runtime_address = runtime_transport.local_addr().unwrap();
        let mut runtime = CallRuntime::udp(
            CallEngine::new(EngineConfig::default()).unwrap(),
            runtime_transport,
        );
        let credentials = DigestCredentials::new("runtime-user", "runtime-secret");
        let (call_id, _) = runtime
            .originate(invite(), peer.local_addr().unwrap(), Duration::ZERO)
            .unwrap();
        let (originated, _) = peer.recv().unwrap();
        let SipMessage::Request(originated) = originated else {
            panic!("expected originated INVITE");
        };
        peer.send_to(&digest_challenge_for_invite(&originated), runtime_address)
            .unwrap();

        let challenged = runtime
            .receive_once_with_digest_auth(
                Duration::from_millis(1),
                &credentials,
                Some("runtime-cnonce"),
                Some(1),
            )
            .unwrap();
        assert_eq!(challenged.actions().len(), 2);
        assert!(challenged.events().is_empty());
        let (ack, _) = peer.recv().unwrap();
        assert!(matches!(
            ack,
            SipMessage::Request(SipRequest {
                method: SipMethod::Ack,
                ..
            })
        ));
        let (retry, _) = peer.recv().unwrap();
        let SipMessage::Request(retry) = retry else {
            panic!("expected authenticated retry");
        };
        assert_eq!(retry.headers.get("CSeq"), Some("2 INVITE"));
        let authorization =
            sip_auth::DigestAuthorization::parse(retry.headers.get("Authorization").unwrap())
                .unwrap();
        assert!(authorization.verify_request(
            &credentials,
            "INVITE",
            &retry.request_uri,
            &retry.body,
        ));
        assert_eq!(
            runtime.engine().snapshot(&call_id).unwrap().state,
            CallState::Inviting
        );

        peer.send_to(&response_for_invite(&retry, 200), runtime_address)
            .unwrap();
        let completed = runtime.receive_once(Duration::from_millis(2)).unwrap();
        assert!(
            completed
                .events()
                .iter()
                .any(|event| event.kind == CallEventKind::Answered)
        );
        let (ack, _) = peer.recv().unwrap();
        assert!(matches!(
            ack,
            SipMessage::Request(SipRequest {
                method: SipMethod::Ack,
                ..
            })
        ));
        assert_eq!(
            runtime.engine().snapshot(&call_id).unwrap().state,
            CallState::Answered
        );
        assert!(!format!("{runtime:?}").contains("runtime-secret"));
    }

    #[test]
    fn provider_digest_policy_and_missing_credentials_fail_atomically() {
        let peer = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 4_096).unwrap();
        let runtime_transport = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 4_096).unwrap();
        let runtime_address = runtime_transport.local_addr().unwrap();
        let mut runtime = CallRuntime::udp(
            CallEngine::new(EngineConfig::default()).unwrap(),
            runtime_transport,
        );
        let (call_id, _) = runtime
            .originate(invite(), peer.local_addr().unwrap(), Duration::ZERO)
            .unwrap();
        let (originated, _) = peer.recv().unwrap();
        let SipMessage::Request(originated) = originated else {
            panic!("expected originated INVITE");
        };
        let transaction_count = runtime.engine().transaction_count();
        let resolutions = std::cell::Cell::new(0);
        let mut resolver = |_: &str| {
            resolutions.set(resolutions.get() + 1);
            None
        };

        peer.send_to(&digest_challenge_for_invite(&originated), runtime_address)
            .unwrap();
        let missing_policy = runtime
            .receive_once_with_provider_digest_auth(
                Duration::from_millis(1),
                &AuthenticationPolicy::None,
                &mut resolver,
                Some("policy-cnonce"),
                Some(1),
            )
            .unwrap_err();
        assert!(matches!(
            missing_policy,
            RuntimeError::Engine(EngineError::DigestAuthenticationNotConfigured)
        ));
        assert_eq!(resolutions.get(), 0);
        assert_eq!(runtime.engine().transaction_count(), transaction_count);
        assert_eq!(
            runtime.engine().snapshot(&call_id).unwrap().state,
            CallState::Inviting
        );

        let policy = AuthenticationPolicy::Digest {
            credential_ref: "opaque-provider-reference".to_owned(),
            algorithm: DigestAlgorithm::Md5,
        };
        peer.send_to(&digest_challenge_for_invite(&originated), runtime_address)
            .unwrap();
        let missing_credentials = runtime
            .receive_once_with_provider_digest_auth(
                Duration::from_millis(2),
                &policy,
                &mut resolver,
                Some("policy-cnonce"),
                Some(1),
            )
            .unwrap_err();
        assert!(matches!(
            missing_credentials,
            RuntimeError::Engine(EngineError::DigestCredentialsUnavailable)
        ));
        assert_eq!(resolutions.get(), 1);
        assert_eq!(runtime.engine().transaction_count(), transaction_count);
        assert_eq!(
            runtime.engine().snapshot(&call_id).unwrap().state,
            CallState::Inviting
        );
        assert!(!format!("{policy:?}").contains("opaque-provider-reference"));
        assert!(!format!("{missing_credentials:?}").contains("opaque-provider-reference"));
    }

    #[test]
    fn provider_digest_resolves_rotation_for_stale_nonce_and_preserves_retry_bound() {
        let peer = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 4_096).unwrap();
        let runtime_transport = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 4_096).unwrap();
        let runtime_address = runtime_transport.local_addr().unwrap();
        let mut runtime = CallRuntime::udp(
            CallEngine::new(EngineConfig {
                max_digest_retries_per_call: 2,
                ..EngineConfig::default()
            })
            .unwrap(),
            runtime_transport,
        );
        let (call_id, _) = runtime
            .originate(invite(), peer.local_addr().unwrap(), Duration::ZERO)
            .unwrap();
        let (originated, _) = peer.recv().unwrap();
        let SipMessage::Request(originated) = originated else {
            panic!("expected originated INVITE");
        };
        let policy = AuthenticationPolicy::Digest {
            credential_ref: "rotating-provider-reference".to_owned(),
            algorithm: DigestAlgorithm::Md5,
        };
        let old_credentials = DigestCredentials::new("runtime-user", "old-runtime-secret");
        let rotated_credentials = DigestCredentials::new("runtime-user", "rotated-runtime-secret");
        let resolutions = std::cell::Cell::new(0);
        let mut resolver = |credential_ref: &str| {
            assert_eq!(credential_ref, "rotating-provider-reference");
            let next = resolutions.get() + 1;
            resolutions.set(next);
            match next {
                1 => Some(old_credentials.clone()),
                2 => Some(rotated_credentials.clone()),
                _ => panic!("credential resolver called after the retry bound"),
            }
        };

        let first_value = r#"Digest realm="runtime", nonce="rotation-nonce-1", qop="auth""#;
        peer.send_to(
            &digest_challenge_for_invite_with(&originated, first_value),
            runtime_address,
        )
        .unwrap();
        runtime
            .receive_once_with_provider_digest_auth(
                Duration::from_millis(1),
                &policy,
                &mut resolver,
                Some("rotation-cnonce-1"),
                Some(1),
            )
            .unwrap();
        let first_retry = receive_ack_and_retry(&peer);
        assert_digest_retry(&first_retry, first_value, &old_credentials);

        let stale_value =
            r#"Digest realm="runtime", nonce="rotation-nonce-2", qop="auth", stale=true"#;
        assert!(DigestChallenge::parse(stale_value).unwrap().stale());
        let stale_challenge = digest_challenge_for_invite_with(&first_retry, stale_value);
        peer.send_to(&stale_challenge, runtime_address).unwrap();
        runtime
            .receive_once_with_provider_digest_auth(
                Duration::from_millis(2),
                &policy,
                &mut resolver,
                Some("rotation-cnonce-2"),
                Some(1),
            )
            .unwrap();
        let second_retry = receive_ack_and_retry(&peer);
        assert_eq!(second_retry.headers.get("CSeq"), Some("3 INVITE"));
        assert_digest_retry(&second_retry, stale_value, &rotated_credentials);
        assert_eq!(resolutions.get(), 2);

        replay_duplicate_provider_digest_challenge(
            &mut runtime,
            &peer,
            runtime_address,
            &stale_challenge,
            &mut resolver,
        );
        assert_eq!(resolutions.get(), 2);

        reject_provider_digest_retry(
            &mut runtime,
            &peer,
            runtime_address,
            &policy,
            &mut resolver,
            &second_retry,
        );
        assert_eq!(resolutions.get(), 2);

        complete_provider_digest_call(
            &mut runtime,
            &peer,
            runtime_address,
            &call_id,
            &second_retry,
        );
        let debug = format!("{runtime:?}");
        assert!(!debug.contains("old-runtime-secret"));
        assert!(!debug.contains("rotated-runtime-secret"));
        assert!(!debug.contains("rotating-provider-reference"));
    }

    #[test]
    fn runtime_originates_human_leg_and_connects_bridge_on_success() {
        let (mut runtime, bridge_id, caller_id) = runtime_with_inbound_bridge();
        let human = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 2_048).unwrap();
        let human_address = human.local_addr().unwrap();
        let runtime_address = runtime.transport().local_addr().unwrap();

        let (human_call_id, originated) = runtime
            .originate_human_leg(
                &bridge_id,
                LegId::from_sequence(2),
                invite(),
                human_address,
                Duration::ZERO,
            )
            .unwrap();

        assert!(
            originated
                .bridge_events()
                .iter()
                .any(|event| event.kind == BridgeEventKind::Created)
        );
        assert!(
            originated
                .bridge_events()
                .iter()
                .any(|event| event.kind == BridgeEventKind::HumanConnecting)
        );
        let (message, _) = human.recv().unwrap();
        let SipMessage::Request(outbound_invite) = message else {
            panic!("expected outbound human INVITE");
        };
        human
            .send_to(&response_for_invite(&outbound_invite, 180), runtime_address)
            .unwrap();
        let ringing = runtime.receive_once(Duration::from_millis(10)).unwrap();
        assert!(ringing.bridge_events().is_empty());
        assert_eq!(
            runtime
                .bridge_registry()
                .unwrap()
                .snapshot(&bridge_id)
                .unwrap()
                .state,
            BridgeState::ConnectingHuman
        );

        human
            .send_to(&response_for_invite(&outbound_invite, 200), runtime_address)
            .unwrap();
        let answered = runtime.receive_once(Duration::from_millis(20)).unwrap();
        assert_eq!(
            answered.bridge_events()[0].kind,
            BridgeEventKind::HumanConnected
        );
        let (ack, _) = human.recv().unwrap();
        assert!(matches!(
            ack,
            SipMessage::Request(SipRequest {
                method: SipMethod::Ack,
                ..
            })
        ));
        let bridge = runtime
            .bridge_registry()
            .unwrap()
            .snapshot(&bridge_id)
            .unwrap();
        assert_eq!(bridge.state, BridgeState::HumanActive);
        assert_eq!(bridge.caller_call_id, caller_id);
        assert_eq!(bridge.active_human.unwrap().call_id, human_call_id);
    }

    #[test]
    fn failed_human_response_restores_ai_and_ends_outbound_call() {
        let (mut runtime, bridge_id, _) = runtime_with_inbound_bridge();
        let human = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 2_048).unwrap();
        let runtime_address = runtime.transport().local_addr().unwrap();
        let (human_call_id, _) = runtime
            .originate_human_leg(
                &bridge_id,
                LegId::from_sequence(2),
                invite(),
                human.local_addr().unwrap(),
                Duration::ZERO,
            )
            .unwrap();
        let (message, _) = human.recv().unwrap();
        let SipMessage::Request(outbound_invite) = message else {
            panic!("expected outbound human INVITE");
        };

        human
            .send_to(&response_for_invite(&outbound_invite, 486), runtime_address)
            .unwrap();
        let failed = runtime.receive_once(Duration::from_millis(10)).unwrap();

        assert!(failed.events().iter().any(|event| {
            event.call_id == human_call_id && event.kind == CallEventKind::Failed
        }));
        assert_eq!(failed.bridge_events()[0].kind, BridgeEventKind::HumanFailed);
        assert_eq!(
            runtime
                .bridge_registry()
                .unwrap()
                .snapshot(&bridge_id)
                .unwrap()
                .state,
            BridgeState::AiActive
        );
        assert_eq!(
            runtime.engine().snapshot(&human_call_id).unwrap().state,
            CallState::Ended
        );
        let (ack, _) = human.recv().unwrap();
        assert!(matches!(
            ack,
            SipMessage::Request(SipRequest {
                method: SipMethod::Ack,
                ..
            })
        ));
    }

    #[test]
    fn human_leg_timeout_restores_ai_routing() {
        let (mut runtime, bridge_id, _) = runtime_with_inbound_bridge();
        let human = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 2_048).unwrap();
        let (human_call_id, _) = runtime
            .originate_human_leg(
                &bridge_id,
                LegId::from_sequence(2),
                invite(),
                human.local_addr().unwrap(),
                Duration::ZERO,
            )
            .unwrap();
        let _ = human.recv().unwrap();

        let timed_out = runtime.poll(Duration::from_secs(33)).unwrap();

        assert!(timed_out.events().iter().any(|event| {
            event.call_id == human_call_id && event.kind == CallEventKind::Failed
        }));
        assert_eq!(
            timed_out.bridge_events()[0].kind,
            BridgeEventKind::HumanFailed
        );
        assert_eq!(
            runtime
                .bridge_registry()
                .unwrap()
                .snapshot(&bridge_id)
                .unwrap()
                .state,
            BridgeState::AiActive
        );
    }

    #[test]
    fn remote_human_bye_restores_ai_routing() {
        let (mut runtime, bridge_id, caller_id) = runtime_with_inbound_bridge();
        let human = UdpTransport::bind("127.0.0.1:0".parse().unwrap(), 2_048).unwrap();
        let runtime_address = runtime.transport().local_addr().unwrap();
        let (human_call_id, _) = runtime
            .originate_human_leg(
                &bridge_id,
                LegId::from_sequence(2),
                invite(),
                human.local_addr().unwrap(),
                Duration::ZERO,
            )
            .unwrap();
        let (message, _) = human.recv().unwrap();
        let SipMessage::Request(outbound_invite) = message else {
            panic!("expected outbound human INVITE");
        };
        human
            .send_to(&response_for_invite(&outbound_invite, 200), runtime_address)
            .unwrap();
        let _ = runtime.receive_once(Duration::from_millis(10)).unwrap();
        let _ = human.recv().unwrap();

        human
            .send_to(&bye_for_invite(&outbound_invite), runtime_address)
            .unwrap();
        let hung_up = runtime.receive_once(Duration::from_millis(20)).unwrap();

        assert_eq!(
            hung_up.bridge_events()[0].kind,
            BridgeEventKind::HumanFailed
        );
        let bridge = runtime
            .bridge_registry()
            .unwrap()
            .snapshot(&bridge_id)
            .unwrap();
        assert_eq!(bridge.state, BridgeState::AiActive);
        assert_eq!(bridge.caller_call_id, caller_id);
        assert_eq!(
            runtime.engine().snapshot(&human_call_id).unwrap().state,
            CallState::Ended
        );
        let (response, _) = human.recv().unwrap();
        assert!(matches!(
            response,
            SipMessage::Response(SipResponse {
                status_code: 200,
                ..
            })
        ));
    }

    #[test]
    fn rejected_human_leg_is_atomic_and_sends_no_invite() {
        let (mut runtime, bridge_id, _) = runtime_with_inbound_bridge();
        runtime
            .bridge_registry_mut()
            .unwrap()
            .end(&bridge_id)
            .unwrap();
        let calls_before = runtime.engine().list(10).unwrap();
        let human = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        human
            .set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();

        let error = runtime
            .originate_human_leg(
                &bridge_id,
                LegId::from_sequence(2),
                invite(),
                human.local_addr().unwrap(),
                Duration::ZERO,
            )
            .unwrap_err();

        assert!(matches!(
            error,
            RuntimeError::Bridge(BridgeError::InvalidOperation { .. })
        ));
        assert_eq!(runtime.engine().list(10).unwrap(), calls_before);
        assert_eq!(
            runtime
                .bridge_registry()
                .unwrap()
                .snapshot(&bridge_id)
                .unwrap()
                .state,
            BridgeState::Ended
        );
        let mut buffer = [0_u8; 1];
        let receive_error = human.recv_from(&mut buffer).unwrap_err();
        assert!(matches!(
            receive_error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ));
    }
}
