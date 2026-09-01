#![allow(clippy::doc_markdown)]

//! Bounded provider profiles and explicit SIP routing policy.
//!
//! Profiles contain provider compatibility policy, not credentials. Secret
//! material is resolved by the caller through the credential reference used by
//! the SIP authentication layer. New profiles default to Asterisk so that
//! adding a Rust implementation cannot silently change production traffic.

use std::{
    error::Error,
    fmt::{self, Debug, Display, Formatter},
};

use sip_auth::DigestAlgorithm;

const DEFAULT_MAX_PROFILES: usize = 64;
const DEFAULT_MAX_DOMAINS_PER_PROFILE: usize = 16;
const DEFAULT_MAX_CODECS_PER_PROFILE: usize = 16;
const DEFAULT_MAX_STRING_BYTES: usize = 512;

/// SIP signaling transports supported by a provider profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SignalingTransport {
    /// SIP over an unreliable datagram.
    Udp,
    /// SIP over a reliable stream.
    Tcp,
    /// SIP over TLS.
    Tls,
    /// SIP over a WebSocket transport.
    WebSocket,
}

/// Media encryption policy advertised to or required by a provider.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaEncryption {
    /// Plain RTP. This is the default for local/demo profiles.
    None,
    /// DTLS-SRTP media.
    DtlsSrtp {
        /// Whether the peer certificate must be verified.
        verify_peer: bool,
    },
    /// SDES-SRTP media.
    Sdes,
}

/// Audio codecs a provider profile may offer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioCodec {
    /// G.711 mu-law, payload type 0.
    Pcmu,
    /// G.711 A-law, payload type 8.
    Pcma,
    /// G.722 wideband audio.
    G722,
    /// Opus wideband audio.
    Opus,
}

impl AudioCodec {
    /// Returns the SDP codec name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pcmu => "PCMU",
            Self::Pcma => "PCMA",
            Self::G722 => "G722",
            Self::Opus => "opus",
        }
    }
}

/// Authentication policy for one provider profile.
#[derive(Clone, Eq, PartialEq)]
pub enum AuthenticationPolicy {
    /// The provider does not require SIP Digest credentials.
    None,
    /// Resolve the named secret at runtime and use SIP Digest authentication.
    Digest {
        /// Secret-store key, never the password itself.
        credential_ref: String,
        /// Digest algorithm required by the provider.
        algorithm: DigestAlgorithm,
    },
}

impl Debug for AuthenticationPolicy {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => formatter.write_str("None"),
            Self::Digest {
                credential_ref,
                algorithm,
            } => formatter
                .debug_struct("Digest")
                .field("credential_ref", &"[CONFIGURED]")
                .field("algorithm", algorithm)
                .field("credential_ref_bytes", &credential_ref.len())
                .finish(),
        }
    }
}

/// NAT and media-path compatibility flags for a provider.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NatPolicy {
    /// Learn the RTP source from received packets.
    pub symmetric_rtp: bool,
    /// Accept and advertise the source port from the SIP transport.
    pub force_rport: bool,
    /// Rewrite Contact with the observed source address.
    pub rewrite_contact: bool,
    /// Allow direct media between call legs.
    pub direct_media: bool,
}

/// Whether a route is handled by the Rust engine or the Asterisk fallback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineTarget {
    /// Route the call through the Rust engine.
    Rust,
    /// Route the call through Asterisk.
    Asterisk,
}

/// Bounds applied to provider profiles and route tables.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RoutingConfig {
    /// Maximum number of provider profiles retained.
    pub max_profiles: usize,
    /// Maximum number of inbound domains on one profile.
    pub max_domains_per_profile: usize,
    /// Maximum number of offered codecs on one profile.
    pub max_codecs_per_profile: usize,
    /// Maximum byte length of one profile string field.
    pub max_string_bytes: usize,
    /// Target used when no profile matches. Defaults to Asterisk.
    pub default_target: EngineTarget,
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            max_profiles: DEFAULT_MAX_PROFILES,
            max_domains_per_profile: DEFAULT_MAX_DOMAINS_PER_PROFILE,
            max_codecs_per_profile: DEFAULT_MAX_CODECS_PER_PROFILE,
            max_string_bytes: DEFAULT_MAX_STRING_BYTES,
            default_target: EngineTarget::Asterisk,
        }
    }
}

impl RoutingConfig {
    fn validate(self) -> Result<Self, RoutingError> {
        if self.max_profiles == 0
            || self.max_domains_per_profile == 0
            || self.max_codecs_per_profile == 0
            || self.max_string_bytes == 0
        {
            return Err(RoutingError::InvalidConfig);
        }
        Ok(self)
    }
}

/// A provider's signaling, media, authentication, and routing compatibility
/// policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderProfile {
    id: String,
    match_domains: Vec<String>,
    signaling_host: String,
    signaling_port: u16,
    signaling_transport: SignalingTransport,
    advertised_address: Option<String>,
    media_encryption: MediaEncryption,
    require_media_encryption: bool,
    codecs: Vec<AudioCodec>,
    authentication: AuthenticationPolicy,
    nat: NatPolicy,
    allow_early_media: bool,
    require_100rel: bool,
    primary_target: EngineTarget,
    fallback_target: EngineTarget,
}

impl ProviderProfile {
    /// Creates a profile with safe Asterisk-first defaults.
    ///
    /// # Errors
    ///
    /// Returns an error when the identity, host, or port is invalid.
    pub fn new(
        id: impl Into<String>,
        signaling_host: impl Into<String>,
        signaling_port: u16,
        signaling_transport: SignalingTransport,
    ) -> Result<Self, RoutingError> {
        let profile = Self {
            id: id.into(),
            match_domains: Vec::new(),
            signaling_host: signaling_host.into(),
            signaling_port,
            signaling_transport,
            advertised_address: None,
            media_encryption: MediaEncryption::None,
            require_media_encryption: false,
            codecs: vec![AudioCodec::Pcmu, AudioCodec::Pcma],
            authentication: AuthenticationPolicy::None,
            nat: NatPolicy::default(),
            allow_early_media: true,
            require_100rel: false,
            primary_target: EngineTarget::Asterisk,
            fallback_target: EngineTarget::Asterisk,
        };
        profile.validate(RoutingConfig::default())?;
        Ok(profile)
    }

    /// Validates this profile against route-table bounds.
    ///
    /// # Errors
    ///
    /// Returns RoutingError when a profile field, codec, authentication, or
    /// rollback policy is invalid.
    pub fn validate(&self, config: RoutingConfig) -> Result<(), RoutingError> {
        let config = config.validate()?;
        validate_identifier(&self.id, "profile id", config.max_string_bytes)?;
        validate_host(
            &self.signaling_host,
            "signaling host",
            config.max_string_bytes,
        )?;
        if self.signaling_port == 0 {
            return Err(RoutingError::InvalidPort);
        }
        if let Some(address) = &self.advertised_address {
            validate_host(address, "advertised address", config.max_string_bytes)?;
        }
        if self.match_domains.len() > config.max_domains_per_profile {
            return Err(RoutingError::TooManyDomains {
                maximum: config.max_domains_per_profile,
            });
        }
        for domain in &self.match_domains {
            validate_domain(domain, config.max_string_bytes)?;
        }
        if self.codecs.is_empty() {
            return Err(RoutingError::NoCodecs);
        }
        if self.codecs.len() > config.max_codecs_per_profile {
            return Err(RoutingError::TooManyCodecs {
                maximum: config.max_codecs_per_profile,
            });
        }
        for (index, codec) in self.codecs.iter().enumerate() {
            if self.codecs[..index].contains(codec) {
                return Err(RoutingError::DuplicateCodec(*codec));
            }
        }
        if let AuthenticationPolicy::Digest {
            credential_ref,
            algorithm,
        } = &self.authentication
        {
            validate_identifier(
                credential_ref,
                "credential reference",
                config.max_string_bytes,
            )?;
            if *algorithm != DigestAlgorithm::Md5 {
                return Err(RoutingError::UnsupportedDigestAlgorithm);
            }
        }
        if self.require_media_encryption && self.media_encryption == MediaEncryption::None {
            return Err(RoutingError::MediaEncryptionRequired);
        }
        if self.fallback_target != EngineTarget::Asterisk {
            return Err(RoutingError::FallbackMustBeAsterisk);
        }
        if self.primary_target == EngineTarget::Rust
            && self.fallback_target != EngineTarget::Asterisk
        {
            return Err(RoutingError::FallbackMustBeAsterisk);
        }
        Ok(())
    }

    /// Adds an inbound From-domain match.
    #[must_use]
    pub fn with_match_domain(mut self, domain: impl Into<String>) -> Self {
        self.match_domains.push(domain.into().to_ascii_lowercase());
        self
    }

    /// Sets the advertised SIP address used in Contact/SDP policy.
    #[must_use]
    pub fn with_advertised_address(mut self, address: impl Into<String>) -> Self {
        self.advertised_address = Some(address.into());
        self
    }

    /// Replaces the offered codec list.
    #[must_use]
    pub fn with_codecs(mut self, codecs: impl IntoIterator<Item = AudioCodec>) -> Self {
        self.codecs = codecs.into_iter().collect();
        self
    }

    /// Adds one codec to the offered codec list.
    #[must_use]
    pub fn with_codec(mut self, codec: AudioCodec) -> Self {
        self.codecs.push(codec);
        self
    }

    /// Sets the authentication policy.
    #[must_use]
    pub fn with_authentication(mut self, authentication: AuthenticationPolicy) -> Self {
        self.authentication = authentication;
        self
    }

    /// Sets media encryption and whether it is mandatory.
    #[must_use]
    pub fn with_media_encryption(
        mut self,
        media_encryption: MediaEncryption,
        required: bool,
    ) -> Self {
        self.media_encryption = media_encryption;
        self.require_media_encryption = required;
        self
    }

    /// Sets NAT and direct-media compatibility flags.
    #[must_use]
    pub fn with_nat_policy(mut self, nat: NatPolicy) -> Self {
        self.nat = nat;
        self
    }

    /// Sets early-media and 100rel behavior.
    #[must_use]
    pub fn with_provisional_policy(
        mut self,
        allow_early_media: bool,
        require_100rel: bool,
    ) -> Self {
        self.allow_early_media = allow_early_media;
        self.require_100rel = require_100rel;
        self
    }

    /// Sets the primary engine and its mandatory Asterisk rollback target.
    #[must_use]
    pub fn with_targets(mut self, primary: EngineTarget, fallback: EngineTarget) -> Self {
        self.primary_target = primary;
        self.fallback_target = fallback;
        self
    }

    /// Returns the profile identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns inbound From-domain matches.
    #[must_use]
    pub fn match_domains(&self) -> &[String] {
        &self.match_domains
    }

    /// Returns the provider signaling host.
    #[must_use]
    pub fn signaling_host(&self) -> &str {
        &self.signaling_host
    }

    /// Returns the provider signaling port.
    #[must_use]
    pub const fn signaling_port(&self) -> u16 {
        self.signaling_port
    }

    /// Returns the signaling transport.
    #[must_use]
    pub const fn signaling_transport(&self) -> SignalingTransport {
        self.signaling_transport
    }

    /// Returns the optional advertised address.
    #[must_use]
    pub fn advertised_address(&self) -> Option<&str> {
        self.advertised_address.as_deref()
    }

    /// Returns the media encryption policy.
    #[must_use]
    pub const fn media_encryption(&self) -> MediaEncryption {
        self.media_encryption
    }

    /// Returns whether media encryption is mandatory.
    #[must_use]
    pub const fn requires_media_encryption(&self) -> bool {
        self.require_media_encryption
    }

    /// Returns the configured codecs.
    #[must_use]
    pub fn codecs(&self) -> &[AudioCodec] {
        &self.codecs
    }

    /// Returns the authentication policy.
    #[must_use]
    pub fn authentication(&self) -> &AuthenticationPolicy {
        &self.authentication
    }

    /// Returns the NAT policy.
    #[must_use]
    pub const fn nat_policy(&self) -> NatPolicy {
        self.nat
    }

    /// Returns whether early media is permitted.
    #[must_use]
    pub const fn allows_early_media(&self) -> bool {
        self.allow_early_media
    }

    /// Returns whether 100rel is mandatory.
    #[must_use]
    pub const fn requires_100rel(&self) -> bool {
        self.require_100rel
    }

    /// Returns the primary routing target.
    #[must_use]
    pub const fn primary_target(&self) -> EngineTarget {
        self.primary_target
    }

    /// Returns the rollback routing target.
    #[must_use]
    pub const fn fallback_target(&self) -> EngineTarget {
        self.fallback_target
    }
}

/// How a route-table lookup matched a profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RouteMatch {
    /// The inbound source matched a configured From domain.
    InboundDomain,
    /// The outbound provider identifier matched a profile.
    OutboundProvider,
    /// No profile matched and the default route was selected.
    Default,
}

/// A routing result carrying the primary target and explicit rollback target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteDecision {
    profile_id: Option<String>,
    target: EngineTarget,
    fallback: EngineTarget,
    matched_by: RouteMatch,
}

impl RouteDecision {
    /// Returns the matched profile identifier, if any.
    #[must_use]
    pub fn profile_id(&self) -> Option<&str> {
        self.profile_id.as_deref()
    }

    /// Returns the selected primary target.
    #[must_use]
    pub const fn target(&self) -> EngineTarget {
        self.target
    }

    /// Returns the explicit rollback target.
    #[must_use]
    pub const fn fallback(&self) -> EngineTarget {
        self.fallback
    }

    /// Returns the reason for the route selection.
    #[must_use]
    pub const fn matched_by(&self) -> RouteMatch {
        self.matched_by
    }
}

/// A bounded provider-profile route table.
#[derive(Clone, Debug)]
pub struct ProviderRouteTable {
    config: RoutingConfig,
    profiles: Vec<ProviderProfile>,
}

impl ProviderRouteTable {
    /// Creates an empty route table with validated bounds.
    ///
    /// # Errors
    ///
    /// Returns RoutingError when a configured bound is zero.
    pub fn new(config: RoutingConfig) -> Result<Self, RoutingError> {
        Ok(Self {
            config: config.validate()?,
            profiles: Vec::new(),
        })
    }

    /// Returns the route-table configuration.
    #[must_use]
    pub const fn config(&self) -> RoutingConfig {
        self.config
    }

    /// Returns the number of retained profiles.
    #[must_use]
    pub fn len(&self) -> usize {
        self.profiles.len()
    }

    /// Returns whether no profiles are configured.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }

    /// Adds one provider profile without replacing an existing identifier.
    ///
    /// # Errors
    ///
    /// Returns RoutingError when the profile is invalid, duplicated, or the
    /// table has reached its configured bound.
    pub fn insert(&mut self, profile: ProviderProfile) -> Result<(), RoutingError> {
        profile.validate(self.config)?;
        if self.profiles.len() >= self.config.max_profiles {
            return Err(RoutingError::TooManyProfiles {
                maximum: self.config.max_profiles,
            });
        }
        if self
            .profiles
            .iter()
            .any(|existing| existing.id.eq_ignore_ascii_case(&profile.id))
        {
            return Err(RoutingError::DuplicateProfile(profile.id));
        }
        for domain in &profile.match_domains {
            if self.profiles.iter().any(|existing| {
                existing
                    .match_domains
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(domain))
            }) {
                return Err(RoutingError::DuplicateDomain(domain.clone()));
            }
        }
        self.profiles.push(profile);
        Ok(())
    }

    /// Returns a profile by identifier, case-insensitively.
    #[must_use]
    pub fn profile(&self, id: &str) -> Option<&ProviderProfile> {
        self.profiles
            .iter()
            .find(|profile| profile.id.eq_ignore_ascii_case(id))
    }

    /// Routes an inbound call using the From domain.
    #[must_use]
    pub fn route_inbound(&self, from_domain: &str) -> RouteDecision {
        if let Some(profile) = self.profiles.iter().find(|profile| {
            profile
                .match_domains
                .iter()
                .any(|domain| domain.eq_ignore_ascii_case(from_domain))
        }) {
            return decision_for(profile, RouteMatch::InboundDomain);
        }
        self.default_decision()
    }

    /// Routes an outbound call using the provider identifier.
    #[must_use]
    pub fn route_outbound(&self, provider_id: &str) -> RouteDecision {
        self.profile(provider_id).map_or_else(
            || self.default_decision(),
            |profile| decision_for(profile, RouteMatch::OutboundProvider),
        )
    }

    /// Looks up either an inbound or outbound route.
    #[must_use]
    pub fn route(&self, request: RouteRequest<'_>) -> RouteDecision {
        match request {
            RouteRequest::Inbound { from_domain } => self.route_inbound(from_domain),
            RouteRequest::Outbound { provider_id } => self.route_outbound(provider_id),
        }
    }

    fn default_decision(&self) -> RouteDecision {
        RouteDecision {
            profile_id: None,
            target: self.config.default_target,
            fallback: EngineTarget::Asterisk,
            matched_by: RouteMatch::Default,
        }
    }
}

fn decision_for(profile: &ProviderProfile, matched_by: RouteMatch) -> RouteDecision {
    RouteDecision {
        profile_id: Some(profile.id.clone()),
        target: profile.primary_target,
        fallback: profile.fallback_target,
        matched_by,
    }
}

/// A route lookup request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RouteRequest<'a> {
    /// Match an inbound call by source From domain.
    Inbound {
        /// Source domain from the SIP identity.
        from_domain: &'a str,
    },
    /// Match an outbound call by configured provider identifier.
    Outbound {
        /// Provider profile identifier.
        provider_id: &'a str,
    },
}

/// Errors returned by profile validation and route-table operations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RoutingError {
    /// One or more route-table bounds were zero.
    InvalidConfig,
    /// A required string was empty.
    EmptyField(&'static str),
    /// A string exceeded its configured bound.
    FieldTooLong {
        /// Name of the field.
        field: &'static str,
        /// Configured maximum byte length.
        maximum: usize,
    },
    /// An identifier contained characters outside the SIP-safe token set.
    InvalidIdentifier(&'static str),
    /// A host/address contained whitespace or a control byte.
    InvalidHost(&'static str),
    /// A provider domain contained invalid syntax.
    InvalidDomain,
    /// A signaling port was zero.
    InvalidPort,
    /// The profile has no usable audio codec.
    NoCodecs,
    /// The profile has more codecs than the configured bound.
    TooManyCodecs {
        /// Configured maximum codec count.
        maximum: usize,
    },
    /// A codec was listed more than once.
    DuplicateCodec(AudioCodec),
    /// A digest credential reference was unsupported or empty.
    UnsupportedDigestAlgorithm,
    /// Encryption was required while the profile selected plain RTP.
    MediaEncryptionRequired,
    /// Every route must retain Asterisk as a rollback target.
    FallbackMustBeAsterisk,
    /// The profile table has reached its configured bound.
    TooManyProfiles {
        /// Configured maximum profile count.
        maximum: usize,
    },
    /// A profile identifier is already present.
    DuplicateProfile(String),
    /// An inbound domain is already claimed by another profile.
    DuplicateDomain(String),
    /// A profile has more inbound domains than the configured bound.
    TooManyDomains {
        /// Configured maximum domain count.
        maximum: usize,
    },
}

impl Display for RoutingError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig => formatter.write_str("provider routing bounds must be non-zero"),
            Self::EmptyField(field) => write!(formatter, "provider {field} must not be empty"),
            Self::FieldTooLong { field, maximum } => {
                write!(
                    formatter,
                    "provider {field} exceeds the {maximum}-byte limit"
                )
            }
            Self::InvalidIdentifier(field) => {
                write!(formatter, "provider {field} is not a SIP-safe identifier")
            }
            Self::InvalidHost(field) => {
                write!(
                    formatter,
                    "provider {field} contains whitespace or control bytes"
                )
            }
            Self::InvalidDomain => formatter.write_str("provider match domain is invalid"),
            Self::InvalidPort => formatter.write_str("provider signaling port must be non-zero"),
            Self::NoCodecs => formatter.write_str("provider profile must offer an audio codec"),
            Self::TooManyCodecs { maximum } => {
                write!(
                    formatter,
                    "provider profile exceeds the {maximum}-codec limit"
                )
            }
            Self::DuplicateCodec(codec) => {
                write!(
                    formatter,
                    "provider profile repeats codec {}",
                    codec.as_str()
                )
            }
            Self::UnsupportedDigestAlgorithm => {
                formatter.write_str("provider profile requires unsupported SIP Digest")
            }
            Self::MediaEncryptionRequired => {
                formatter.write_str("provider profile requires media encryption")
            }
            Self::FallbackMustBeAsterisk => {
                formatter.write_str("provider profile fallback must remain Asterisk")
            }
            Self::TooManyProfiles { maximum } => {
                write!(
                    formatter,
                    "provider route table exceeds the {maximum}-profile limit"
                )
            }
            Self::DuplicateProfile(id) => write!(formatter, "provider profile {id} already exists"),
            Self::DuplicateDomain(domain) => {
                write!(formatter, "provider domain {domain} is already claimed")
            }
            Self::TooManyDomains { maximum } => {
                write!(
                    formatter,
                    "provider profile exceeds the {maximum}-domain limit"
                )
            }
        }
    }
}

impl Error for RoutingError {}

fn validate_identifier(
    value: &str,
    field: &'static str,
    maximum: usize,
) -> Result<(), RoutingError> {
    if value.is_empty() {
        return Err(RoutingError::EmptyField(field));
    }
    if value.len() > maximum {
        return Err(RoutingError::FieldTooLong { field, maximum });
    }
    if !value.bytes().all(is_token_byte) {
        return Err(RoutingError::InvalidIdentifier(field));
    }
    Ok(())
}

fn validate_host(value: &str, field: &'static str, maximum: usize) -> Result<(), RoutingError> {
    if value.is_empty() {
        return Err(RoutingError::EmptyField(field));
    }
    if value.len() > maximum {
        return Err(RoutingError::FieldTooLong { field, maximum });
    }
    if value
        .bytes()
        .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
        || value
            .bytes()
            .any(|byte| matches!(byte, b'/' | b'@' | b';' | b'<' | b'>' | b'\"'))
    {
        return Err(RoutingError::InvalidHost(field));
    }
    Ok(())
}

fn validate_domain(value: &str, maximum: usize) -> Result<(), RoutingError> {
    if value.is_empty() {
        return Err(RoutingError::EmptyField("match domain"));
    }
    if value.len() > maximum {
        return Err(RoutingError::FieldTooLong {
            field: "match domain",
            maximum,
        });
    }
    if value
        .bytes()
        .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
        || value
            .bytes()
            .any(|byte| matches!(byte, b'/' | b'@' | b';' | b'<' | b'>' | b'\"'))
    {
        return Err(RoutingError::InvalidDomain);
    }
    Ok(())
}

fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'..=b'\'' | b'*'..=b'+' | b'-'..=b'.' | b'^' | b'_' | b'|' | b'~'
        )
        || byte == 96
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_captures_provider_policy_without_secrets() {
        let profile =
            ProviderProfile::new("meta-wa", "sip-trunk.w3.run", 5061, SignalingTransport::Tls)
                .unwrap()
                .with_match_domain("WA.META.VC")
                .with_advertised_address("195.201.246.125")
                .with_codecs([
                    AudioCodec::Pcmu,
                    AudioCodec::Pcma,
                    AudioCodec::G722,
                    AudioCodec::Opus,
                ])
                .with_authentication(AuthenticationPolicy::Digest {
                    credential_ref: "meta-wa-credentials".to_owned(),
                    algorithm: DigestAlgorithm::Md5,
                })
                .with_media_encryption(MediaEncryption::DtlsSrtp { verify_peer: false }, true)
                .with_nat_policy(NatPolicy {
                    symmetric_rtp: true,
                    force_rport: true,
                    rewrite_contact: true,
                    direct_media: false,
                })
                .with_provisional_policy(true, false)
                .with_targets(EngineTarget::Rust, EngineTarget::Asterisk);

        assert!(profile.validate(RoutingConfig::default()).is_ok());
        assert_eq!(profile.match_domains(), &["wa.meta.vc".to_owned()]);
        assert_eq!(
            profile.media_encryption(),
            MediaEncryption::DtlsSrtp { verify_peer: false }
        );
        assert_eq!(profile.primary_target(), EngineTarget::Rust);
        assert_eq!(profile.fallback_target(), EngineTarget::Asterisk);
        let debug = format!("{profile:?}");
        assert!(!debug.contains("meta-wa-credentials"));
        assert!(debug.contains("[CONFIGURED]"));
    }

    #[test]
    fn route_table_is_bounded_and_keeps_asterisk_default_and_fallback() {
        let mut table = ProviderRouteTable::new(RoutingConfig {
            max_profiles: 1,
            ..RoutingConfig::default()
        })
        .unwrap();
        let profile =
            ProviderProfile::new("meta-wa", "sip-trunk.w3.run", 5061, SignalingTransport::Tls)
                .unwrap()
                .with_match_domain("wa.meta.vc")
                .with_targets(EngineTarget::Rust, EngineTarget::Asterisk);
        table.insert(profile).unwrap();

        let inbound = table.route_inbound("WA.META.VC");
        assert_eq!(inbound.profile_id(), Some("meta-wa"));
        assert_eq!(inbound.target(), EngineTarget::Rust);
        assert_eq!(inbound.fallback(), EngineTarget::Asterisk);
        assert_eq!(inbound.matched_by(), RouteMatch::InboundDomain);

        let outbound = table.route_outbound("META-WA");
        assert_eq!(outbound.profile_id(), Some("meta-wa"));
        assert_eq!(outbound.matched_by(), RouteMatch::OutboundProvider);

        let unknown = table.route_inbound("unknown.example");
        assert_eq!(unknown.profile_id(), None);
        assert_eq!(unknown.target(), EngineTarget::Asterisk);
        assert_eq!(unknown.fallback(), EngineTarget::Asterisk);
        assert_eq!(unknown.matched_by(), RouteMatch::Default);

        let second =
            ProviderProfile::new("local", "localhost", 5060, SignalingTransport::Udp).unwrap();
        assert!(matches!(
            table.insert(second),
            Err(RoutingError::TooManyProfiles { maximum: 1 })
        ));
    }

    #[test]
    fn validation_rejects_unsafe_or_ambiguous_policy() {
        assert!(matches!(
            ProviderProfile::new("bad id", "host", 5060, SignalingTransport::Udp),
            Err(RoutingError::InvalidIdentifier("profile id"))
        ));
        assert!(matches!(
            ProviderProfile::new(
                "provider",
                "host;transport=tls",
                5060,
                SignalingTransport::Udp
            ),
            Err(RoutingError::InvalidHost("signaling host"))
        ));
        let no_encryption = ProviderProfile::new("provider", "host", 5060, SignalingTransport::Udp)
            .unwrap()
            .with_media_encryption(MediaEncryption::None, true);
        assert!(matches!(
            no_encryption.validate(RoutingConfig::default()),
            Err(RoutingError::MediaEncryptionRequired)
        ));
        let bad_fallback = ProviderProfile::new("provider", "host", 5060, SignalingTransport::Udp)
            .unwrap()
            .with_targets(EngineTarget::Rust, EngineTarget::Rust);
        assert!(matches!(
            bad_fallback.validate(RoutingConfig::default()),
            Err(RoutingError::FallbackMustBeAsterisk)
        ));
    }

    #[test]
    fn duplicate_profiles_domains_and_codecs_are_rejected() {
        let mut table = ProviderRouteTable::new(RoutingConfig::default()).unwrap();
        let first = ProviderProfile::new("one", "host", 5060, SignalingTransport::Udp)
            .unwrap()
            .with_match_domain("example.com");
        table.insert(first).unwrap();
        let duplicate_domain = ProviderProfile::new("two", "host", 5060, SignalingTransport::Udp)
            .unwrap()
            .with_match_domain("EXAMPLE.COM");
        assert!(matches!(
            table.insert(duplicate_domain),
            Err(RoutingError::DuplicateDomain(_))
        ));
        let duplicate_codec = ProviderProfile::new("three", "host", 5060, SignalingTransport::Udp)
            .unwrap()
            .with_codecs([AudioCodec::Pcmu, AudioCodec::Pcmu]);
        assert!(matches!(
            table.insert(duplicate_codec),
            Err(RoutingError::DuplicateCodec(AudioCodec::Pcmu))
        ));
        let duplicate_id =
            ProviderProfile::new("ONE", "other", 5061, SignalingTransport::Tcp).unwrap();
        assert!(matches!(
            table.insert(duplicate_id),
            Err(RoutingError::DuplicateProfile(_))
        ));
    }

    #[test]
    fn zero_bounds_fail_before_table_creation() {
        assert!(matches!(
            ProviderRouteTable::new(RoutingConfig {
                max_profiles: 0,
                ..RoutingConfig::default()
            }),
            Err(RoutingError::InvalidConfig)
        ));
    }
}
