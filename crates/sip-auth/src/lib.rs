#![allow(clippy::doc_markdown)]

//! Bounded SIP Digest authentication primitives.
//!
//! This crate deliberately handles only the MD5 variants that are widely used
//! by SIP providers. It does not own sockets, credentials, or a clock. Callers
//! provide the request method, URI, body, and a monotonic Duration when using
//! AuthFailureThrottle.

use std::{
    collections::HashMap,
    error::Error,
    fmt::{self, Debug, Display, Formatter, Write as _},
    time::Duration,
};

const MD5_HEX_LEN: usize = 32;
const DEFAULT_MAX_AUTH_BYTES: usize = 8_192;
const DEFAULT_MAX_PARAMETERS: usize = 32;
const DEFAULT_MAX_PARAMETER_BYTES: usize = 2_048;
const DEFAULT_MAX_IDENTITIES: usize = 4_096;
const DEFAULT_MAX_FAILURES: u32 = 5;
const DEFAULT_EXPIRY: Duration = Duration::from_secs(60);

/// Bounds applied while parsing one Digest challenge or authorization value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DigestParseConfig {
    /// Maximum number of bytes in the complete header value.
    pub max_bytes: usize,
    /// Maximum number of comma-separated parameters.
    pub max_parameters: usize,
    /// Maximum number of bytes in one parameter, including its name and value.
    pub max_parameter_bytes: usize,
}

impl Default for DigestParseConfig {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_MAX_AUTH_BYTES,
            max_parameters: DEFAULT_MAX_PARAMETERS,
            max_parameter_bytes: DEFAULT_MAX_PARAMETER_BYTES,
        }
    }
}

impl DigestParseConfig {
    fn validate(self) -> Result<Self, DigestError> {
        if self.max_bytes == 0 || self.max_parameters == 0 || self.max_parameter_bytes == 0 {
            return Err(DigestError::InvalidConfig);
        }
        Ok(self)
    }
}

/// Supported SIP Digest algorithms.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DigestAlgorithm {
    /// RFC 2617 MD5.
    Md5,
}

impl DigestAlgorithm {
    /// Returns the wire spelling of this algorithm.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Md5 => "MD5",
        }
    }
}

/// Quality-of-protection modes supported by this implementation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DigestQop {
    /// Authenticate the request method and URI.
    Auth,
    /// Authenticate the method, URI, and entity body.
    AuthInt,
}

impl DigestQop {
    /// Returns the wire spelling of this quality-of-protection mode.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auth => "auth",
            Self::AuthInt => "auth-int",
        }
    }
}

/// A parsed WWW-Authenticate or Proxy-Authenticate Digest challenge.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DigestChallenge {
    realm: String,
    nonce: String,
    opaque: Option<String>,
    algorithm: DigestAlgorithm,
    qop: Option<DigestQop>,
    stale: bool,
}

impl DigestChallenge {
    /// Parses a Digest challenge using default bounds.
    ///
    /// # Errors
    ///
    /// Returns DigestError when the value is malformed or exceeds a bound.
    pub fn parse(value: &str) -> Result<Self, DigestError> {
        Self::parse_with_config(value, DigestParseConfig::default())
    }

    /// Parses a Digest challenge with explicit resource bounds.
    ///
    /// # Errors
    ///
    /// Returns DigestError when the value or supplied bounds are invalid.
    pub fn parse_with_config(value: &str, config: DigestParseConfig) -> Result<Self, DigestError> {
        let fields = parse_digest_fields(value, config)?;
        let realm = required_field(&fields, "realm")?;
        let nonce = required_field(&fields, "nonce")?;
        let algorithm = parse_algorithm(fields.get("algorithm"))?;
        let qop = parse_qop(fields.get("qop"))?;
        let stale = parse_bool(fields.get("stale"))?.unwrap_or(false);

        Ok(Self {
            realm,
            nonce,
            opaque: fields.get("opaque").cloned(),
            algorithm,
            qop,
            stale,
        })
    }

    /// Creates an MD5 challenge without qop or opaque parameters.
    #[must_use]
    pub fn new(realm: impl Into<String>, nonce: impl Into<String>) -> Self {
        Self {
            realm: realm.into(),
            nonce: nonce.into(),
            opaque: None,
            algorithm: DigestAlgorithm::Md5,
            qop: None,
            stale: false,
        }
    }

    /// Sets the optional opaque challenge value.
    #[must_use]
    pub fn with_opaque(mut self, opaque: impl Into<String>) -> Self {
        self.opaque = Some(opaque.into());
        self
    }

    /// Sets the qop offered by this challenge.
    #[must_use]
    pub fn with_qop(mut self, qop: DigestQop) -> Self {
        self.qop = Some(qop);
        self
    }

    /// Marks this challenge as a stale-nonce retry.
    #[must_use]
    pub fn with_stale(mut self, stale: bool) -> Self {
        self.stale = stale;
        self
    }

    /// Returns the challenge realm.
    #[must_use]
    pub fn realm(&self) -> &str {
        &self.realm
    }

    /// Returns the challenge nonce.
    #[must_use]
    pub fn nonce(&self) -> &str {
        &self.nonce
    }

    /// Returns the optional opaque value.
    #[must_use]
    pub fn opaque(&self) -> Option<&str> {
        self.opaque.as_deref()
    }

    /// Returns the selected algorithm.
    #[must_use]
    pub const fn algorithm(&self) -> DigestAlgorithm {
        self.algorithm
    }

    /// Returns the selected qop, if any.
    #[must_use]
    pub const fn qop(&self) -> Option<DigestQop> {
        self.qop
    }

    /// Returns whether the challenge marks the nonce stale.
    #[must_use]
    pub const fn stale(&self) -> bool {
        self.stale
    }

    /// Serializes this challenge as a SIP Digest value.
    #[must_use]
    pub fn to_header_value(&self) -> String {
        let mut value = format!(
            "Digest realm=\"{}\", nonce=\"{}\", algorithm={}",
            escape_quoted(&self.realm),
            escape_quoted(&self.nonce),
            self.algorithm.as_str()
        );
        if let Some(opaque) = &self.opaque {
            value.push_str(", opaque=\"");
            value.push_str(&escape_quoted(opaque));
            value.push('\"');
        }
        if let Some(qop) = self.qop {
            value.push_str(", qop=\"");
            value.push_str(qop.as_str());
            value.push('\"');
        }
        if self.stale {
            value.push_str(", stale=true");
        }
        value
    }
}

/// Parsed SIP Authorization or Proxy-Authorization Digest credentials.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DigestAuthorization {
    username: String,
    realm: String,
    nonce: String,
    uri: String,
    response: String,
    algorithm: DigestAlgorithm,
    opaque: Option<String>,
    qop: Option<DigestQop>,
    cnonce: Option<String>,
    nonce_count: Option<u32>,
}

impl DigestAuthorization {
    /// Parses an authorization value using default bounds.
    ///
    /// # Errors
    ///
    /// Returns DigestError when the value is malformed or exceeds a bound.
    pub fn parse(value: &str) -> Result<Self, DigestError> {
        Self::parse_with_config(value, DigestParseConfig::default())
    }

    /// Parses an authorization value with explicit resource bounds.
    ///
    /// # Errors
    ///
    /// Returns DigestError when the value or supplied bounds are invalid.
    pub fn parse_with_config(value: &str, config: DigestParseConfig) -> Result<Self, DigestError> {
        let fields = parse_digest_fields(value, config)?;
        let username = required_field(&fields, "username")?;
        let realm = required_field(&fields, "realm")?;
        let nonce = required_field(&fields, "nonce")?;
        let uri = required_field(&fields, "uri")?;
        let response = required_field(&fields, "response")?;
        validate_response(&response)?;
        let algorithm = parse_algorithm(fields.get("algorithm"))?;
        let qop = parse_qop_value(fields.get("qop"))?;
        let cnonce = fields.get("cnonce").cloned();
        let nonce_count = fields
            .get("nc")
            .map(|value| parse_nonce_count(value))
            .transpose()?;

        if qop.is_some() != (cnonce.is_some() && nonce_count.is_some()) {
            return Err(DigestError::MissingParameter("cnonce/nc"));
        }
        if qop.is_none() && (cnonce.is_some() || nonce_count.is_some()) {
            return Err(DigestError::UnexpectedParameter("cnonce/nc"));
        }

        Ok(Self {
            username,
            realm,
            nonce,
            uri,
            response,
            algorithm,
            opaque: fields.get("opaque").cloned(),
            qop,
            cnonce,
            nonce_count,
        })
    }

    /// Builds an authorization response from a username/password pair.
    ///
    /// # Errors
    ///
    /// Returns DigestError when the challenge or qop inputs are incomplete.
    #[allow(clippy::too_many_arguments)]
    pub fn from_credentials(
        credentials: &DigestCredentials,
        challenge: &DigestChallenge,
        method: &str,
        uri: &str,
        body: &[u8],
        cnonce: Option<&str>,
        nonce_count: Option<u32>,
    ) -> Result<Self, DigestError> {
        if method.is_empty() || uri.is_empty() {
            return Err(DigestError::EmptyParameter);
        }
        if challenge.algorithm != DigestAlgorithm::Md5 {
            return Err(DigestError::UnsupportedAlgorithm);
        }
        let (qop, cnonce, nonce_count) = if let Some(qop) = challenge.qop {
            let cnonce = cnonce
                .filter(|value| !value.is_empty())
                .ok_or(DigestError::MissingParameter("cnonce"))?;
            let nonce_count = nonce_count.ok_or(DigestError::MissingParameter("nc"))?;
            (Some(qop), Some(cnonce.to_owned()), Some(nonce_count))
        } else {
            if cnonce.is_some() || nonce_count.is_some() {
                return Err(DigestError::UnexpectedParameter("cnonce/nc"));
            }
            (None, None, None)
        };

        let response = calculate_response(
            credentials,
            &challenge.realm,
            &challenge.nonce,
            method,
            uri,
            body,
            qop,
            cnonce.as_deref(),
            nonce_count,
        );
        Ok(Self {
            username: credentials.username.clone(),
            realm: challenge.realm.clone(),
            nonce: challenge.nonce.clone(),
            uri: uri.to_owned(),
            response,
            algorithm: challenge.algorithm,
            opaque: challenge.opaque.clone(),
            qop,
            cnonce,
            nonce_count,
        })
    }

    /// Returns the username asserted by the peer.
    #[must_use]
    pub fn username(&self) -> &str {
        &self.username
    }

    /// Returns the authorization realm.
    #[must_use]
    pub fn realm(&self) -> &str {
        &self.realm
    }

    /// Returns the nonce used for this response.
    #[must_use]
    pub fn nonce(&self) -> &str {
        &self.nonce
    }

    /// Returns the request URI covered by this response.
    #[must_use]
    pub fn uri(&self) -> &str {
        &self.uri
    }

    /// Returns the hexadecimal response digest.
    #[must_use]
    pub fn response(&self) -> &str {
        &self.response
    }

    /// Returns the algorithm used for this response.
    #[must_use]
    pub const fn algorithm(&self) -> DigestAlgorithm {
        self.algorithm
    }

    /// Returns the optional opaque challenge value.
    #[must_use]
    pub fn opaque(&self) -> Option<&str> {
        self.opaque.as_deref()
    }

    /// Returns the qop used for this response.
    #[must_use]
    pub const fn qop(&self) -> Option<DigestQop> {
        self.qop
    }

    /// Returns the client nonce when qop is in use.
    #[must_use]
    pub fn cnonce(&self) -> Option<&str> {
        self.cnonce.as_deref()
    }

    /// Returns the parsed hexadecimal nonce count.
    #[must_use]
    pub const fn nonce_count(&self) -> Option<u32> {
        self.nonce_count
    }

    /// Verifies this authorization against the supplied credentials and SIP
    /// request. The response digest is compared in constant time.
    #[must_use]
    pub fn verify(&self, credentials: &DigestCredentials, method: &str, body: &[u8]) -> bool {
        if self.algorithm != DigestAlgorithm::Md5 || self.username != credentials.username {
            return false;
        }
        let expected = calculate_response(
            credentials,
            &self.realm,
            &self.nonce,
            method,
            &self.uri,
            body,
            self.qop,
            self.cnonce.as_deref(),
            self.nonce_count,
        );
        constant_time_hex_eq(&expected, &self.response)
    }

    /// Verifies this authorization against a specific challenge, credentials,
    /// and SIP request. This additionally binds the response to the challenge
    /// realm, nonce, opaque value, algorithm, and qop.
    #[must_use]
    pub fn verify_against(
        &self,
        challenge: &DigestChallenge,
        credentials: &DigestCredentials,
        method: &str,
        body: &[u8],
    ) -> bool {
        self.realm == challenge.realm
            && self.nonce == challenge.nonce
            && self.opaque == challenge.opaque
            && self.algorithm == challenge.algorithm
            && self.qop == challenge.qop
            && self.verify(credentials, method, body)
    }

    /// Serializes this authorization as a SIP Digest value.
    #[must_use]
    pub fn to_header_value(&self) -> String {
        let mut value = format!(
            "Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{}\", response=\"{}\"",
            escape_quoted(&self.username),
            escape_quoted(&self.realm),
            escape_quoted(&self.nonce),
            escape_quoted(&self.uri),
            self.response,
        );
        value.push_str(", algorithm=");
        value.push_str(self.algorithm.as_str());
        if let Some(opaque) = &self.opaque {
            value.push_str(", opaque=\"");
            value.push_str(&escape_quoted(opaque));
            value.push('\"');
        }
        if let (Some(qop), Some(cnonce), Some(nonce_count)) =
            (self.qop, self.cnonce.as_deref(), self.nonce_count)
        {
            value.push_str(", qop=");
            value.push_str(qop.as_str());
            value.push_str(", nc=");
            write!(&mut value, "{nonce_count:08x}").expect("writing to String cannot fail");
            value.push_str(", cnonce=\"");
            value.push_str(&escape_quoted(cnonce));
            value.push('\"');
        }
        value
    }
}

/// Username and password used to calculate a SIP Digest response.
///
/// The password is intentionally redacted from Debug output. Callers should
/// still avoid logging this value or putting it in metrics.
#[derive(Clone, Eq, PartialEq)]
pub struct DigestCredentials {
    username: String,
    password: String,
}

impl DigestCredentials {
    /// Creates credentials for one SIP identity.
    #[must_use]
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            password: password.into(),
        }
    }

    /// Returns the username. The password has no public getter by design.
    #[must_use]
    pub fn username(&self) -> &str {
        &self.username
    }
}

impl Debug for DigestCredentials {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DigestCredentials")
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .finish()
    }
}

/// Errors returned by Digest parsing, response construction, or validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DigestError {
    /// A parser or throttle bound was zero.
    InvalidConfig,
    /// The complete value exceeded its configured bound.
    ValueTooLarge {
        /// Supplied byte length.
        actual: usize,
        /// Configured maximum byte length.
        maximum: usize,
    },
    /// A parameter exceeded its configured bound.
    ParameterTooLarge {
        /// Configured maximum parameter byte length.
        maximum: usize,
    },
    /// Too many comma-separated parameters were supplied.
    TooManyParameters {
        /// Configured maximum parameter count.
        maximum: usize,
    },
    /// The value did not start with the Digest scheme.
    InvalidScheme,
    /// A parameter was malformed or had no value.
    InvalidParameter,
    /// A required parameter was absent.
    MissingParameter(&'static str),
    /// A parameter was supplied where this qop mode does not permit it.
    UnexpectedParameter(&'static str),
    /// A required parameter was empty.
    EmptyParameter,
    /// The algorithm is not supported by this crate.
    UnsupportedAlgorithm,
    /// A response was not exactly 32 hexadecimal characters.
    InvalidResponse,
    /// A nonce count was not eight hexadecimal characters.
    InvalidNonceCount,
    /// A boolean parameter was not true or false.
    InvalidBoolean,
    /// The failure throttle bound was reached.
    ThrottleConfigInvalid,
}

impl Display for DigestError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig => formatter.write_str("Digest bounds must be non-zero"),
            Self::ValueTooLarge { actual, maximum } => {
                write!(
                    formatter,
                    "Digest value is {actual} bytes, maximum is {maximum}"
                )
            }
            Self::ParameterTooLarge { maximum } => {
                write!(
                    formatter,
                    "Digest parameter exceeds the {maximum}-byte limit"
                )
            }
            Self::TooManyParameters { maximum } => {
                write!(
                    formatter,
                    "Digest value exceeds the {maximum}-parameter limit"
                )
            }
            Self::InvalidScheme => formatter.write_str("Digest value must use the Digest scheme"),
            Self::InvalidParameter => formatter.write_str("Digest parameter is malformed"),
            Self::MissingParameter(name) => write!(formatter, "Digest parameter {name} is missing"),
            Self::UnexpectedParameter(name) => {
                write!(
                    formatter,
                    "Digest parameter {name} is not valid in this mode"
                )
            }
            Self::EmptyParameter => formatter.write_str("Digest parameter must not be empty"),
            Self::UnsupportedAlgorithm => formatter.write_str("Digest algorithm is unsupported"),
            Self::InvalidResponse => {
                formatter.write_str("Digest response is not 32 hex characters")
            }
            Self::InvalidNonceCount => {
                formatter.write_str("Digest nonce count must be 8 hex characters")
            }
            Self::InvalidBoolean => formatter.write_str("Digest boolean must be true or false"),
            Self::ThrottleConfigInvalid => {
                formatter.write_str("Digest throttle bounds must be non-zero")
            }
        }
    }
}

impl Error for DigestError {}

/// Bounds for per-identity failed-authentication throttling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuthThrottleConfig {
    /// Maximum number of identity entries retained at once.
    pub max_identities: usize,
    /// Number of failures allowed during one expiry interval.
    pub max_failures: u32,
    /// Time after which an identity entry expires and its count resets.
    pub expiry: Duration,
}

impl Default for AuthThrottleConfig {
    fn default() -> Self {
        Self {
            max_identities: DEFAULT_MAX_IDENTITIES,
            max_failures: DEFAULT_MAX_FAILURES,
            expiry: DEFAULT_EXPIRY,
        }
    }
}

impl AuthThrottleConfig {
    fn validate(self) -> Result<Self, DigestError> {
        if self.max_identities == 0 || self.max_failures == 0 || self.expiry.is_zero() {
            return Err(DigestError::ThrottleConfigInvalid);
        }
        Ok(self)
    }
}

#[derive(Clone, Debug)]
struct FailureEntry {
    failures: u32,
    expires_at: Duration,
}

/// Result of checking an identity against the authentication-failure limit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthThrottleDecision {
    /// Authentication may proceed.
    Allowed,
    /// Authentication should be rejected until the supplied duration elapses.
    Throttled {
        /// Remaining time in the identity's expiry interval.
        retry_after: Duration,
    },
}

/// Bounded per-identity authentication failure tracking.
#[derive(Clone, Debug)]
pub struct AuthFailureThrottle {
    config: AuthThrottleConfig,
    entries: HashMap<String, FailureEntry>,
}

impl AuthFailureThrottle {
    /// Creates an empty throttle with validated bounds.
    ///
    /// # Errors
    ///
    /// Returns DigestError::ThrottleConfigInvalid for zero bounds.
    pub fn new(config: AuthThrottleConfig) -> Result<Self, DigestError> {
        Ok(Self {
            config: config.validate()?,
            entries: HashMap::new(),
        })
    }

    /// Returns the configured throttle bounds.
    #[must_use]
    pub const fn config(&self) -> AuthThrottleConfig {
        self.config
    }

    /// Returns the number of retained identity entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether no identity entries are retained.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Removes expired entries. Supplying a monotonic time before an entry's
    /// expiry leaves the entry untouched.
    pub fn expire(&mut self, now: Duration) {
        self.entries.retain(|_, entry| entry.expires_at > now);
    }

    /// Checks whether an identity is currently throttled.
    pub fn check(&mut self, identity: &str, now: Duration) -> AuthThrottleDecision {
        self.expire(now);
        match self.entries.get(identity) {
            Some(entry) if entry.failures >= self.config.max_failures => {
                AuthThrottleDecision::Throttled {
                    retry_after: entry.expires_at.saturating_sub(now),
                }
            }
            _ => AuthThrottleDecision::Allowed,
        }
    }

    /// Records one failed authentication and returns the resulting decision.
    pub fn record_failure(
        &mut self,
        identity: impl Into<String>,
        now: Duration,
    ) -> AuthThrottleDecision {
        self.expire(now);
        let identity = identity.into();
        if let Some(entry) = self.entries.get_mut(&identity) {
            entry.failures = entry.failures.saturating_add(1);
            return if entry.failures >= self.config.max_failures {
                AuthThrottleDecision::Throttled {
                    retry_after: entry.expires_at.saturating_sub(now),
                }
            } else {
                AuthThrottleDecision::Allowed
            };
        }

        if self.entries.len() >= self.config.max_identities {
            // Keep the structure bounded even when an attacker rotates identity
            // strings. Evict the earliest-expiring entry before admitting one.
            if let Some(eviction_key) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.expires_at)
                .map(|(key, _)| key.clone())
            {
                self.entries.remove(&eviction_key);
            }
        }
        let expires_at = now.saturating_add(self.config.expiry);
        let throttled = self.config.max_failures <= 1;
        self.entries.insert(
            identity,
            FailureEntry {
                failures: 1,
                expires_at,
            },
        );
        if throttled {
            AuthThrottleDecision::Throttled {
                retry_after: self.config.expiry,
            }
        } else {
            AuthThrottleDecision::Allowed
        }
    }

    /// Clears failed-authentication state after a successful authentication.
    pub fn record_success(&mut self, identity: &str) {
        self.entries.remove(identity);
    }
}

fn parse_digest_fields(
    input: &str,
    config: DigestParseConfig,
) -> Result<HashMap<String, String>, DigestError> {
    let config = config.validate()?;
    if input.len() > config.max_bytes {
        return Err(DigestError::ValueTooLarge {
            actual: input.len(),
            maximum: config.max_bytes,
        });
    }
    let input = input.trim();
    let rest = input
        .get(..6)
        .filter(|scheme| scheme.eq_ignore_ascii_case("Digest"))
        .map(|_| &input[6..])
        .ok_or(DigestError::InvalidScheme)?;
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return Err(DigestError::InvalidScheme);
    }

    let mut fields = HashMap::new();
    let mut parameters = 0;
    for part in split_parameters(rest.trim())? {
        parameters += 1;
        if parameters > config.max_parameters {
            return Err(DigestError::TooManyParameters {
                maximum: config.max_parameters,
            });
        }
        if part.len() > config.max_parameter_bytes {
            return Err(DigestError::ParameterTooLarge {
                maximum: config.max_parameter_bytes,
            });
        }
        let (name, value) = part.split_once('=').ok_or(DigestError::InvalidParameter)?;
        let name = name.trim();
        if name.is_empty() || !name.bytes().all(is_token_byte) {
            return Err(DigestError::InvalidParameter);
        }
        let value = parse_parameter_value(value.trim())?;
        if value.is_empty() {
            return Err(DigestError::EmptyParameter);
        }
        let name = name.to_ascii_lowercase();
        if fields.insert(name, value).is_some() {
            return Err(DigestError::InvalidParameter);
        }
    }
    if fields.is_empty() {
        return Err(DigestError::InvalidParameter);
    }
    Ok(fields)
}

fn split_parameters(input: &str) -> Result<Vec<&str>, DigestError> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    for (index, byte) in input.bytes().enumerate() {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else if byte == b'"' {
            quoted = true;
        } else if byte == b',' {
            let part = input[start..index].trim();
            if part.is_empty() {
                return Err(DigestError::InvalidParameter);
            }
            parts.push(part);
            start = index + 1;
        }
    }
    if quoted || escaped {
        return Err(DigestError::InvalidParameter);
    }
    let part = input[start..].trim();
    if part.is_empty() {
        return Err(DigestError::InvalidParameter);
    }
    parts.push(part);
    Ok(parts)
}

fn parse_parameter_value(value: &str) -> Result<String, DigestError> {
    if let Some(value) = value.strip_prefix('"') {
        let Some(value) = value.strip_suffix('"') else {
            return Err(DigestError::InvalidParameter);
        };
        let mut output = String::with_capacity(value.len());
        let mut escaped = false;
        for byte in value.bytes() {
            if escaped {
                if byte != b'"' && byte != b'\\' {
                    return Err(DigestError::InvalidParameter);
                }
                output.push(byte as char);
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' || byte.is_ascii_control() {
                return Err(DigestError::InvalidParameter);
            } else {
                output.push(byte as char);
            }
        }
        if escaped {
            return Err(DigestError::InvalidParameter);
        }
        Ok(output)
    } else if value.is_empty()
        || value
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
        || !value.bytes().all(is_token_byte)
    {
        Err(DigestError::InvalidParameter)
    } else {
        Ok(value.to_owned())
    }
}

fn required_field(
    fields: &HashMap<String, String>,
    name: &'static str,
) -> Result<String, DigestError> {
    fields
        .get(name)
        .cloned()
        .ok_or(DigestError::MissingParameter(name))
}

fn parse_algorithm(value: Option<&String>) -> Result<DigestAlgorithm, DigestError> {
    match value.map_or("MD5", String::as_str) {
        value if value.eq_ignore_ascii_case("MD5") => Ok(DigestAlgorithm::Md5),
        _ => Err(DigestError::UnsupportedAlgorithm),
    }
}

fn parse_qop(value: Option<&String>) -> Result<Option<DigestQop>, DigestError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let mut selected = None;
    for token in value.split(',').map(str::trim) {
        let candidate = if token.eq_ignore_ascii_case("auth") {
            Some(DigestQop::Auth)
        } else if token.eq_ignore_ascii_case("auth-int") {
            Some(DigestQop::AuthInt)
        } else {
            None
        };
        if let Some(candidate) = candidate {
            // Prefer auth when a server offers both, because it avoids hashing
            // a potentially large body while remaining RFC 2617 compatible.
            if selected.is_none() || candidate == DigestQop::Auth {
                selected = Some(candidate);
            }
        } else if !token.is_empty() {
            return Err(DigestError::UnsupportedAlgorithm);
        }
    }
    selected.ok_or(DigestError::UnsupportedAlgorithm).map(Some)
}

fn parse_qop_value(value: Option<&String>) -> Result<Option<DigestQop>, DigestError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.eq_ignore_ascii_case("auth") {
        Ok(Some(DigestQop::Auth))
    } else if value.eq_ignore_ascii_case("auth-int") {
        Ok(Some(DigestQop::AuthInt))
    } else {
        Err(DigestError::UnsupportedAlgorithm)
    }
}

fn parse_bool(value: Option<&String>) -> Result<Option<bool>, DigestError> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value.to_ascii_lowercase().as_str() {
        "true" => Ok(Some(true)),
        "false" => Ok(Some(false)),
        _ => Err(DigestError::InvalidBoolean),
    }
}

fn parse_nonce_count(value: &str) -> Result<u32, DigestError> {
    if value.len() != 8 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(DigestError::InvalidNonceCount);
    }
    u32::from_str_radix(value, 16).map_err(|_| DigestError::InvalidNonceCount)
}

fn validate_response(response: &str) -> Result<(), DigestError> {
    if response.len() != MD5_HEX_LEN || !response.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(DigestError::InvalidResponse);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn calculate_response(
    credentials: &DigestCredentials,
    realm: &str,
    nonce: &str,
    method: &str,
    uri: &str,
    body: &[u8],
    qop: Option<DigestQop>,
    cnonce: Option<&str>,
    nonce_count: Option<u32>,
) -> String {
    let ha1 = md5_hex(&format!(
        "{}:{}:{}",
        credentials.username, realm, credentials.password
    ));
    let entity_hash = md5_hex_bytes(body);
    let ha2 = match qop {
        Some(DigestQop::AuthInt) => md5_hex(&format!("{method}:{uri}:{entity_hash}")),
        _ => md5_hex(&format!("{method}:{uri}")),
    };
    let response = match qop {
        Some(qop) => format!(
            "{}:{}:{:08x}:{}:{}:{}",
            ha1,
            nonce,
            nonce_count.unwrap_or_default(),
            cnonce.unwrap_or_default(),
            qop.as_str(),
            ha2
        ),
        None => format!("{ha1}:{nonce}:{ha2}"),
    };
    md5_hex(&response)
}

fn md5_hex(value: &str) -> String {
    md5_hex_bytes(value.as_bytes())
}

fn md5_hex_bytes(value: &[u8]) -> String {
    format!("{:x}", md5::compute(value))
}

/// Compares two hexadecimal MD5 responses without an early exit.
#[must_use]
pub fn constant_time_hex_eq(expected: &str, actual: &str) -> bool {
    let mut difference = expected.len() ^ actual.len();
    for index in 0..MD5_HEX_LEN {
        let left = expected.as_bytes().get(index).copied().unwrap_or(0);
        let right = actual.as_bytes().get(index).copied().unwrap_or(0);
        difference |= usize::from(left.to_ascii_lowercase() ^ right.to_ascii_lowercase());
    }
    difference == 0 && expected.len() == MD5_HEX_LEN && actual.len() == MD5_HEX_LEN
}

fn escape_quoted(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
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
    fn rfc_2617_known_vector_matches() {
        let challenge = DigestChallenge::parse(
            r#"Digest realm="testrealm@host.com", qop="auth,auth-int", nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093", opaque="5ccc069c403ebaf9f0171e9517f40e41""#,
        )
        .unwrap();
        let credentials = DigestCredentials::new("Mufasa", "Circle Of Life");
        let authorization = DigestAuthorization::from_credentials(
            &credentials,
            &challenge,
            "GET",
            "/dir/index.html",
            b"",
            Some("0a4f113b"),
            Some(1),
        )
        .unwrap();
        assert_eq!(authorization.response(), "6629fae49393a05397450978507c4ef1");
        assert!(authorization.verify(&credentials, "GET", b""));
        assert!(authorization.verify_against(&challenge, &credentials, "GET", b""));
        assert!(!authorization.verify(
            &DigestCredentials::new("other-user", "Circle Of Life"),
            "GET",
            b""
        ));
        assert_eq!(
            DigestAuthorization::parse(&authorization.to_header_value()).unwrap(),
            authorization
        );
        assert_eq!(
            DigestChallenge::parse(&challenge.to_header_value()).unwrap(),
            challenge
        );
    }

    #[test]
    fn auth_int_uses_entity_body_hash() {
        let challenge = DigestChallenge::new("example", "nonce").with_qop(DigestQop::AuthInt);
        let credentials = DigestCredentials::new("alice", "secret");
        let authorization = DigestAuthorization::from_credentials(
            &credentials,
            &challenge,
            "MESSAGE",
            "sip:bob@example.com",
            b"hello",
            Some("cnonce"),
            Some(1),
        )
        .unwrap();
        assert!(authorization.verify(&credentials, "MESSAGE", b"hello"));
        assert!(!authorization.verify(&credentials, "MESSAGE", b"changed"));
    }

    #[test]
    fn parsing_is_bounded_and_rejects_malformed_values() {
        assert_eq!(
            DigestChallenge::parse("digest realm=example, nonce=nonce")
                .unwrap()
                .realm(),
            "example"
        );
        assert!(matches!(
            DigestChallenge::parse_with_config(
                "Digest realm=example, nonce=nonce",
                DigestParseConfig {
                    max_bytes: 8,
                    ..DigestParseConfig::default()
                }
            ),
            Err(DigestError::ValueTooLarge { .. })
        ));
        assert!(matches!(
            DigestChallenge::parse("Digest realm=example, nonce=nonce, stale=maybe"),
            Err(DigestError::InvalidBoolean)
        ));
        assert!(matches!(
            DigestAuthorization::parse(
                "Digest username=alice, realm=example, nonce=nonce, uri=\"sip:bob\", response=bad"
            ),
            Err(DigestError::InvalidResponse)
        ));
    }

    #[test]
    fn credentials_debug_redacts_password() {
        let credentials = DigestCredentials::new("alice", "do-not-log");
        let debug = format!("{credentials:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("do-not-log"));
    }

    #[test]
    fn response_comparison_is_case_insensitive_but_fixed_width() {
        assert!(constant_time_hex_eq(
            "6629fae49393a05397450978507c4ef1",
            "6629FAE49393A05397450978507C4EF1"
        ));
        assert!(!constant_time_hex_eq("abc", "abc"));
    }

    #[test]
    fn throttle_is_bounded_and_expires() {
        let mut throttle = AuthFailureThrottle::new(AuthThrottleConfig {
            max_identities: 2,
            max_failures: 2,
            expiry: Duration::from_secs(10),
        })
        .unwrap();
        assert_eq!(
            throttle.record_failure("alice", Duration::ZERO),
            AuthThrottleDecision::Allowed
        );
        assert_eq!(
            throttle.record_failure("alice", Duration::from_secs(1)),
            AuthThrottleDecision::Throttled {
                retry_after: Duration::from_secs(9)
            }
        );
        assert_eq!(
            throttle.check("alice", Duration::from_secs(2)),
            AuthThrottleDecision::Throttled {
                retry_after: Duration::from_secs(8)
            }
        );
        assert_eq!(
            throttle.check("alice", Duration::from_secs(10)),
            AuthThrottleDecision::Allowed
        );
        throttle.record_failure("bob", Duration::from_secs(10));
        throttle.record_failure("carol", Duration::from_secs(10));
        assert_eq!(throttle.len(), 2);
    }
}
