//! Bounded source-address policy for SIP and RTP ingress.
//!
//! A policy evaluates the observed peer [`IpAddr`] rather than an address
//! supplied by a SIP header. Deny rules are always evaluated first. When no
//! allowlist is configured, an address is allowed unless it matches a deny
//! rule. Once an allowlist is configured, an address must match an allow rule
//! and must not match a deny rule.

use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    str::FromStr,
};

const DEFAULT_MAX_ALLOWLIST_ENTRIES: usize = 64;
const DEFAULT_MAX_DENYLIST_ENTRIES: usize = 64;

/// A canonical IPv4 or IPv6 network in CIDR notation.
///
/// Host bits in the input address are masked during construction, so equal
/// networks have equal values even when written with different host addresses.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Cidr {
    address: IpAddr,
    prefix_len: u8,
}

/// Alias emphasizing that a CIDR is an IP network.
pub type IpCidr = Cidr;

/// Alias for callers that use the common IP-network terminology.
pub type IpNetwork = Cidr;

impl Cidr {
    /// Parses and canonicalizes a CIDR string.
    ///
    /// # Errors
    ///
    /// Returns [`CidrError`] for missing separators, invalid addresses, or a
    /// prefix outside the address family’s range.
    pub fn parse(value: &str) -> Result<Self, CidrError> {
        value.parse()
    }

    /// Creates and canonicalizes a CIDR from an address and prefix length.
    ///
    /// # Errors
    ///
    /// Returns [`CidrError::PrefixTooLong`] when `prefix_len` is larger than
    /// 32 for IPv4 or 128 for IPv6.
    pub fn new(address: IpAddr, prefix_len: u8) -> Result<Self, CidrError> {
        let maximum: u8 = match address {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };
        if prefix_len > maximum {
            return Err(CidrError::PrefixTooLong { maximum });
        }

        let address = match address {
            IpAddr::V4(address) => IpAddr::V4(mask_v4(address, prefix_len)),
            IpAddr::V6(address) => IpAddr::V6(mask_v6(address, prefix_len)),
        };
        Ok(Self {
            address,
            prefix_len,
        })
    }

    /// Returns the canonical network address.
    #[must_use]
    pub const fn network(&self) -> IpAddr {
        self.address
    }

    /// Returns the prefix length.
    #[must_use]
    pub const fn prefix_len(&self) -> u8 {
        self.prefix_len
    }

    /// Returns whether `address` belongs to this network.
    #[must_use]
    pub fn contains(&self, address: IpAddr) -> bool {
        match (self.address, address) {
            (IpAddr::V4(network), IpAddr::V4(address)) => {
                mask_v4(address, self.prefix_len) == network
            }
            (IpAddr::V6(network), IpAddr::V6(address)) => {
                mask_v6(address, self.prefix_len) == network
            }
            _ => false,
        }
    }
}

impl Display for Cidr {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.address, self.prefix_len)
    }
}

impl FromStr for Cidr {
    type Err = CidrError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty() {
            return Err(CidrError::Empty);
        }
        if value
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
        {
            return Err(CidrError::Whitespace);
        }
        let (address, prefix) = value.split_once('/').ok_or(CidrError::MissingPrefix)?;
        if address.is_empty() {
            return Err(CidrError::EmptyAddress);
        }
        if prefix.is_empty() {
            return Err(CidrError::EmptyPrefix);
        }
        let address = address
            .parse::<IpAddr>()
            .map_err(|_| CidrError::InvalidAddress)?;
        let prefix_len = prefix
            .parse::<usize>()
            .map_err(|_| CidrError::InvalidPrefix)?;
        let maximum: u8 = match address {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };
        if prefix_len > usize::from(maximum) {
            return Err(CidrError::PrefixTooLong { maximum });
        }
        let prefix_len = u8::try_from(prefix_len).map_err(|_| CidrError::InvalidPrefix)?;
        Self::new(address, prefix_len)
    }
}

/// Errors returned while parsing or constructing a [`Cidr`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CidrError {
    /// The input string was empty.
    Empty,
    /// The input did not contain a `/` prefix separator.
    MissingPrefix,
    /// The address portion was empty.
    EmptyAddress,
    /// The prefix portion was empty.
    EmptyPrefix,
    /// The address portion was not a valid IPv4 or IPv6 address.
    InvalidAddress,
    /// The prefix portion was not an unsigned decimal integer.
    InvalidPrefix,
    /// Whitespace or a control byte was present in the input.
    Whitespace,
    /// The prefix exceeds the maximum for the address family.
    PrefixTooLong {
        /// Maximum prefix length for the address family.
        maximum: u8,
    },
}

impl Display for CidrError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("CIDR must not be empty"),
            Self::MissingPrefix => formatter.write_str("CIDR must include a prefix length"),
            Self::EmptyAddress => formatter.write_str("CIDR address must not be empty"),
            Self::EmptyPrefix => formatter.write_str("CIDR prefix must not be empty"),
            Self::InvalidAddress => formatter.write_str("CIDR address is invalid"),
            Self::InvalidPrefix => formatter.write_str("CIDR prefix must be an unsigned integer"),
            Self::Whitespace => formatter.write_str("CIDR must not contain whitespace"),
            Self::PrefixTooLong { maximum } => {
                write!(formatter, "CIDR prefix exceeds the {maximum}-bit limit")
            }
        }
    }
}

impl Error for CidrError {}

/// Bounds for one [`SourceIpPolicy`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourcePolicyConfig {
    /// Maximum number of CIDRs in the allowlist.
    pub max_allowlist_entries: usize,
    /// Maximum number of CIDRs in the denylist.
    pub max_denylist_entries: usize,
}

impl Default for SourcePolicyConfig {
    fn default() -> Self {
        Self {
            max_allowlist_entries: DEFAULT_MAX_ALLOWLIST_ENTRIES,
            max_denylist_entries: DEFAULT_MAX_DENYLIST_ENTRIES,
        }
    }
}

impl SourcePolicyConfig {
    fn validate(self) -> Result<Self, PolicyError> {
        if self.max_allowlist_entries == 0 || self.max_denylist_entries == 0 {
            return Err(PolicyError::InvalidConfig);
        }
        Ok(self)
    }
}

/// The result of evaluating an observed source address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceDecision {
    /// The source address passed the configured policy.
    Allow,
    /// The source address was rejected by the configured policy.
    Deny,
}

impl SourceDecision {
    /// Returns the decision as a boolean suitable for an ingress guard.
    #[must_use]
    pub const fn is_allowed(self) -> bool {
        matches!(self, Self::Allow)
    }
}

/// A bounded source IP allow/deny policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceIpPolicy {
    config: SourcePolicyConfig,
    allowlist_configured: bool,
    allowlist: Vec<Cidr>,
    denylist: Vec<Cidr>,
}

/// Short alias for [`SourceIpPolicy`].
pub type SourcePolicy = SourceIpPolicy;

impl SourceIpPolicy {
    /// Creates an empty policy with validated bounds.
    ///
    /// With both lists empty, the policy allows all source addresses. Adding
    /// an allow rule changes the default for non-matching addresses to deny.
    ///
    /// # Errors
    ///
    /// Returns [`PolicyError::InvalidConfig`] when either bound is zero.
    pub fn new(config: SourcePolicyConfig) -> Result<Self, PolicyError> {
        Ok(Self {
            config: config.validate()?,
            allowlist_configured: false,
            allowlist: Vec::new(),
            denylist: Vec::new(),
        })
    }

    /// Creates a policy and inserts the supplied canonical CIDRs.
    ///
    /// # Errors
    ///
    /// Returns a policy error if a list exceeds its bound or contains a
    /// duplicate entry.
    pub fn from_cidrs(
        config: SourcePolicyConfig,
        allowlist: impl IntoIterator<Item = Cidr>,
        denylist: impl IntoIterator<Item = Cidr>,
    ) -> Result<Self, PolicyError> {
        let mut policy = Self::new(config)?;
        // A caller supplying an allowlist is explicitly configuring that
        // boundary, even when the supplied list is empty. This makes an empty
        // configured allowlist fail closed rather than silently allowing all.
        policy.allowlist_configured = true;
        for cidr in allowlist {
            policy.insert_allow(cidr)?;
        }
        for cidr in denylist {
            policy.insert_deny(cidr)?;
        }
        Ok(policy)
    }

    /// Returns the validated policy bounds.
    #[must_use]
    pub const fn config(&self) -> SourcePolicyConfig {
        self.config
    }

    /// Returns the configured allowlist.
    #[must_use]
    pub fn allowlist(&self) -> &[Cidr] {
        &self.allowlist
    }

    /// Returns the configured denylist.
    #[must_use]
    pub fn denylist(&self) -> &[Cidr] {
        &self.denylist
    }

    /// Returns whether at least one allow rule is configured.
    #[must_use]
    pub fn has_allowlist(&self) -> bool {
        self.allowlist_configured
    }

    /// Inserts one canonical CIDR into the allowlist.
    ///
    /// # Errors
    ///
    /// Returns a duplicate or bound error when the entry cannot be retained.
    pub fn insert_allow(&mut self, cidr: Cidr) -> Result<(), PolicyError> {
        if self.allowlist.contains(&cidr) {
            return Err(PolicyError::DuplicateAllowlist(cidr));
        }
        if self.allowlist.len() >= self.config.max_allowlist_entries {
            return Err(PolicyError::AllowlistFull {
                maximum: self.config.max_allowlist_entries,
            });
        }
        self.allowlist_configured = true;
        self.allowlist.push(cidr);
        Ok(())
    }

    /// Replaces the allowlist and marks it as explicitly configured.
    ///
    /// Passing an empty iterator intentionally denies every source that is not
    /// denied first, because an explicitly configured empty allowlist is a
    /// fail-closed policy.
    ///
    /// # Errors
    ///
    /// Returns a duplicate or bound error when an entry cannot be retained.
    pub fn set_allowlist(
        &mut self,
        entries: impl IntoIterator<Item = Cidr>,
    ) -> Result<(), PolicyError> {
        let mut replacement = Vec::new();
        for cidr in entries {
            if replacement.contains(&cidr) {
                return Err(PolicyError::DuplicateAllowlist(cidr));
            }
            if replacement.len() >= self.config.max_allowlist_entries {
                return Err(PolicyError::AllowlistFull {
                    maximum: self.config.max_allowlist_entries,
                });
            }
            replacement.push(cidr);
        }
        self.allowlist = replacement;
        self.allowlist_configured = true;
        Ok(())
    }

    /// Removes all allow rules and restores the default-allow behavior.
    pub fn clear_allowlist(&mut self) {
        self.allowlist.clear();
        self.allowlist_configured = false;
    }

    /// Parses and inserts one CIDR into the allowlist.
    ///
    /// # Errors
    ///
    /// Returns [`PolicyError::InvalidCidr`] for malformed input or a policy
    /// error when the entry cannot be retained.
    pub fn add_allow(&mut self, cidr: &str) -> Result<(), PolicyError> {
        let cidr = cidr.parse().map_err(PolicyError::InvalidCidr)?;
        self.insert_allow(cidr)
    }

    /// Inserts one canonical CIDR into the denylist.
    ///
    /// # Errors
    ///
    /// Returns a duplicate or bound error when the entry cannot be retained.
    pub fn insert_deny(&mut self, cidr: Cidr) -> Result<(), PolicyError> {
        if self.denylist.contains(&cidr) {
            return Err(PolicyError::DuplicateDenylist(cidr));
        }
        if self.denylist.len() >= self.config.max_denylist_entries {
            return Err(PolicyError::DenylistFull {
                maximum: self.config.max_denylist_entries,
            });
        }
        self.denylist.push(cidr);
        Ok(())
    }

    /// Parses and inserts one CIDR into the denylist.
    ///
    /// # Errors
    ///
    /// Returns [`PolicyError::InvalidCidr`] for malformed input or a policy
    /// error when the entry cannot be retained.
    pub fn add_deny(&mut self, cidr: &str) -> Result<(), PolicyError> {
        let cidr = cidr.parse().map_err(PolicyError::InvalidCidr)?;
        self.insert_deny(cidr)
    }

    /// Evaluates an observed peer address, with deny rules taking precedence.
    #[must_use]
    pub fn decision(&self, source: IpAddr) -> SourceDecision {
        if self.denylist.iter().any(|cidr| cidr.contains(source)) {
            return SourceDecision::Deny;
        }
        if !self.allowlist_configured || self.allowlist.iter().any(|cidr| cidr.contains(source)) {
            SourceDecision::Allow
        } else {
            SourceDecision::Deny
        }
    }

    /// Returns whether an observed peer address is allowed.
    #[must_use]
    pub fn allows(&self, source: IpAddr) -> bool {
        self.decision(source).is_allowed()
    }

    /// Returns whether an observed socket peer is allowed.
    #[must_use]
    pub fn allows_socket(&self, source: std::net::SocketAddr) -> bool {
        self.allows(source.ip())
    }
}

impl Default for SourceIpPolicy {
    fn default() -> Self {
        Self::new(SourcePolicyConfig::default()).expect("default source policy config is valid")
    }
}

/// Errors returned while constructing or mutating a [`SourceIpPolicy`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PolicyError {
    /// One or more policy bounds were zero.
    InvalidConfig,
    /// A CIDR string could not be parsed.
    InvalidCidr(CidrError),
    /// The canonical CIDR already exists in the allowlist.
    DuplicateAllowlist(Cidr),
    /// The canonical CIDR already exists in the denylist.
    DuplicateDenylist(Cidr),
    /// The allowlist has reached its configured bound.
    AllowlistFull {
        /// Maximum configured allowlist entries.
        maximum: usize,
    },
    /// The denylist has reached its configured bound.
    DenylistFull {
        /// Maximum configured denylist entries.
        maximum: usize,
    },
}

impl Display for PolicyError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig => formatter.write_str("source policy bounds must be non-zero"),
            Self::InvalidCidr(error) => write!(formatter, "invalid source CIDR: {error}"),
            Self::DuplicateAllowlist(cidr) => {
                write!(formatter, "source allowlist already contains {cidr}")
            }
            Self::DuplicateDenylist(cidr) => {
                write!(formatter, "source denylist already contains {cidr}")
            }
            Self::AllowlistFull { maximum } => {
                write!(
                    formatter,
                    "source allowlist exceeds the {maximum}-entry limit"
                )
            }
            Self::DenylistFull { maximum } => {
                write!(
                    formatter,
                    "source denylist exceeds the {maximum}-entry limit"
                )
            }
        }
    }
}

impl Error for PolicyError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidCidr(error) => Some(error),
            _ => None,
        }
    }
}

fn mask_v4(address: Ipv4Addr, prefix_len: u8) -> Ipv4Addr {
    let bits = u32::from(address);
    let mask = if prefix_len == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(prefix_len))
    };
    Ipv4Addr::from(bits & mask)
}

fn mask_v6(address: Ipv6Addr, prefix_len: u8) -> Ipv6Addr {
    let bits = u128::from(address);
    let mask = if prefix_len == 0 {
        0
    } else {
        u128::MAX << (128 - u32::from(prefix_len))
    };
    Ipv6Addr::from(bits & mask)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;

    #[test]
    fn canonicalizes_ipv4_and_checks_boundaries() {
        let cidr: Cidr = "192.0.2.99/24".parse().unwrap();
        assert_eq!(cidr.network(), "192.0.2.0".parse::<IpAddr>().unwrap());
        assert_eq!(cidr.prefix_len(), 24);
        assert!(cidr.contains("192.0.2.0".parse().unwrap()));
        assert!(cidr.contains("192.0.2.255".parse().unwrap()));
        assert!(!cidr.contains("192.0.3.0".parse().unwrap()));
        assert_eq!(cidr.to_string(), "192.0.2.0/24");
    }

    #[test]
    fn canonicalizes_ipv6_and_rejects_other_families() {
        let cidr: Cidr = "2001:db8:abcd:1::42/64".parse().unwrap();
        assert_eq!(
            cidr.network(),
            "2001:db8:abcd:1::".parse::<IpAddr>().unwrap()
        );
        assert!(cidr.contains("2001:db8:abcd:1::ffff".parse().unwrap()));
        assert!(!cidr.contains("2001:db8:abcd:2::1".parse().unwrap()));
        assert!(!cidr.contains("192.0.2.1".parse().unwrap()));
        assert_eq!(cidr.to_string(), "2001:db8:abcd:1::/64");
    }

    #[test]
    fn malformed_cidrs_have_deterministic_errors() {
        assert_eq!("".parse::<Cidr>(), Err(CidrError::Empty));
        assert_eq!("192.0.2.1".parse::<Cidr>(), Err(CidrError::MissingPrefix));
        assert_eq!("192.0.2.1/".parse::<Cidr>(), Err(CidrError::EmptyPrefix));
        assert_eq!(
            "not-an-ip/24".parse::<Cidr>(),
            Err(CidrError::InvalidAddress)
        );
        assert_eq!(
            "192.0.2.1/nope".parse::<Cidr>(),
            Err(CidrError::InvalidPrefix)
        );
        assert_eq!(
            "192.0.2.1/33".parse::<Cidr>(),
            Err(CidrError::PrefixTooLong { maximum: 32 })
        );
        assert_eq!(
            "192.0.2.1/256".parse::<Cidr>(),
            Err(CidrError::PrefixTooLong { maximum: 32 })
        );
        assert_eq!(
            "2001:db8::1/129".parse::<Cidr>(),
            Err(CidrError::PrefixTooLong { maximum: 128 })
        );
        assert_eq!(" 192.0.2.0/24".parse::<Cidr>(), Err(CidrError::Whitespace));
    }

    #[test]
    fn policy_defaults_to_allow_without_an_allowlist() {
        let mut policy = SourceIpPolicy::default();
        assert!(policy.allows("203.0.113.10".parse().unwrap()));
        policy.add_deny("203.0.113.0/24").unwrap();
        assert!(!policy.allows("203.0.113.10".parse().unwrap()));
        assert!(policy.allows("198.51.100.10".parse().unwrap()));
    }

    #[test]
    fn allowlist_changes_default_and_deny_takes_precedence() {
        let mut policy = SourceIpPolicy::default();
        policy.add_allow("198.51.100.0/24").unwrap();
        policy.add_deny("198.51.100.128/25").unwrap();
        assert!(policy.allows("198.51.100.10".parse().unwrap()));
        assert!(!policy.allows("198.51.100.200".parse().unwrap()));
        assert!(!policy.allows("203.0.113.10".parse().unwrap()));
        assert_eq!(
            policy.decision("198.51.100.200".parse().unwrap()),
            SourceDecision::Deny
        );
    }

    #[test]
    fn canonical_duplicates_and_bounds_are_rejected() {
        let config = SourcePolicyConfig {
            max_allowlist_entries: 1,
            max_denylist_entries: 1,
        };
        let mut policy = SourceIpPolicy::new(config).unwrap();
        policy.add_allow("192.0.2.1/24").unwrap();
        assert!(matches!(
            policy.add_allow("192.0.2.0/24"),
            Err(PolicyError::DuplicateAllowlist(_))
        ));
        assert!(matches!(
            policy.add_allow("198.51.100.0/24"),
            Err(PolicyError::AllowlistFull { maximum: 1 })
        ));
        policy.add_deny("2001:db8::/32").unwrap();
        assert!(matches!(
            policy.add_deny("2001:db8:1::/48"),
            Err(PolicyError::DenylistFull { maximum: 1 })
        ));
        assert!(matches!(
            SourceIpPolicy::new(SourcePolicyConfig {
                max_allowlist_entries: 0,
                ..config
            }),
            Err(PolicyError::InvalidConfig)
        ));
    }

    #[test]
    fn from_cidrs_and_socket_helper_preserve_policy() {
        let allow: Cidr = "2001:db8::/32".parse().unwrap();
        let deny: Cidr = "2001:db8:bad::/48".parse().unwrap();
        let policy =
            SourceIpPolicy::from_cidrs(SourcePolicyConfig::default(), [allow], [deny]).unwrap();
        assert!(policy.allows_socket("[2001:db8::10]:5061".parse::<SocketAddr>().unwrap()));
        assert!(!policy.allows_socket("[2001:db8:bad::10]:5061".parse::<SocketAddr>().unwrap()));
    }

    #[test]
    fn explicitly_empty_allowlist_fails_closed_and_can_be_cleared() {
        let mut policy = SourceIpPolicy::default();
        policy.set_allowlist(std::iter::empty()).unwrap();
        assert!(policy.has_allowlist());
        assert!(!policy.allows("198.51.100.10".parse().unwrap()));
        policy.clear_allowlist();
        assert!(!policy.has_allowlist());
        assert!(policy.allows("198.51.100.10".parse().unwrap()));
    }
}
