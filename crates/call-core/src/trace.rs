//! Bounded W3C trace context and call correlation primitives.
//!
//! The call engine is intentionally runtime agnostic and does not depend on
//! an OpenTelemetry SDK.  These types provide the stable, bounded context that
//! an outer OpenTelemetry adapter can turn into spans while preserving the
//! application-owned [`super::CallId`] across SIP, media, and AI boundaries.

use std::{error::Error, fmt::Display};

use super::CallId;

/// Number of bytes in a W3C trace ID.
pub const TRACE_ID_BYTES: usize = 16;
/// Number of bytes in a W3C span ID.
pub const SPAN_ID_BYTES: usize = 8;
/// Maximum operation-name bytes retained by one span.
pub const MAX_TRACE_OPERATION_BYTES: usize = 64;
const TRACEPARENT_BYTES: usize = 55;

/// Errors raised while parsing or constructing bounded trace data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TraceError {
    /// A trace ID was not exactly 16 non-zero bytes encoded as hexadecimal.
    InvalidTraceId,
    /// A span ID was not exactly 8 non-zero bytes encoded as hexadecimal.
    InvalidSpanId,
    /// A traceparent did not use the supported W3C version-00 shape.
    InvalidTraceparent,
    /// A traceparent contained a zero trace or parent span ID.
    ZeroIdentifier,
    /// A child span reused its parent span ID.
    DuplicateSpan,
    /// A span operation name was empty, oversized, or contained controls.
    InvalidOperation,
}

impl Display for TraceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidTraceId => "trace ID must be 32 non-zero hexadecimal characters",
            Self::InvalidSpanId => "span ID must be 16 non-zero hexadecimal characters",
            Self::InvalidTraceparent => "traceparent must use W3C version 00",
            Self::ZeroIdentifier => "trace identifiers must not be zero",
            Self::DuplicateSpan => "child span ID must differ from its parent",
            Self::InvalidOperation => {
                "trace operation must be non-empty printable ASCII within 64 bytes"
            }
        })
    }
}

impl Error for TraceError {}

/// A validated W3C-compatible 128-bit trace identifier.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TraceId([u8; TRACE_ID_BYTES]);

impl TraceId {
    /// Parses a lower- or upper-case hexadecimal trace ID.
    pub fn from_hex(value: &str) -> Result<Self, TraceError> {
        let bytes = decode_hex::<TRACE_ID_BYTES>(value).ok_or(TraceError::InvalidTraceId)?;
        if bytes.iter().all(|byte| *byte == 0) {
            return Err(TraceError::ZeroIdentifier);
        }
        Ok(Self(bytes))
    }

    /// Creates a deterministic non-zero ID for offline call/replay tests.
    ///
    /// Production adapters should replace this root with a cryptographically
    /// random OpenTelemetry ID while retaining the same bounded type.
    #[must_use]
    pub fn from_sequence(sequence: u64) -> Self {
        let mut bytes = [0_u8; TRACE_ID_BYTES];
        bytes[..8].copy_from_slice(&sequence.to_be_bytes());
        bytes[8..].copy_from_slice(&(!sequence).to_be_bytes());
        if bytes.iter().all(|byte| *byte == 0) {
            bytes[TRACE_ID_BYTES - 1] = 1;
        }
        Self(bytes)
    }

    /// Returns the canonical lower-case hexadecimal representation.
    #[must_use]
    pub fn as_hex(&self) -> String {
        encode_hex(&self.0)
    }

    /// Borrows the raw identifier bytes for an SDK adapter.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; TRACE_ID_BYTES] {
        &self.0
    }
}

/// A validated W3C-compatible 64-bit span identifier.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SpanId([u8; SPAN_ID_BYTES]);

impl SpanId {
    /// Parses a lower- or upper-case hexadecimal span ID.
    pub fn from_hex(value: &str) -> Result<Self, TraceError> {
        let bytes = decode_hex::<SPAN_ID_BYTES>(value).ok_or(TraceError::InvalidSpanId)?;
        if bytes.iter().all(|byte| *byte == 0) {
            return Err(TraceError::ZeroIdentifier);
        }
        Ok(Self(bytes))
    }

    /// Creates a deterministic non-zero ID for offline call/replay tests.
    #[must_use]
    pub fn from_sequence(sequence: u64) -> Self {
        let mut bytes = sequence.to_be_bytes();
        if bytes.iter().all(|byte| *byte == 0) {
            bytes[SPAN_ID_BYTES - 1] = 1;
        }
        Self(bytes)
    }

    /// Returns the canonical lower-case hexadecimal representation.
    #[must_use]
    pub fn as_hex(&self) -> String {
        encode_hex(&self.0)
    }

    /// Borrows the raw identifier bytes for an SDK adapter.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; SPAN_ID_BYTES] {
        &self.0
    }
}

/// W3C trace flags retained without exposing unbounded vendor state.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TraceFlags(u8);

impl TraceFlags {
    /// Creates flags from the two hexadecimal characters in a traceparent.
    pub fn from_hex(value: &str) -> Result<Self, TraceError> {
        if value.len() != 2 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(TraceError::InvalidTraceparent);
        }
        let high = hex_value(value.as_bytes()[0]).ok_or(TraceError::InvalidTraceparent)?;
        let low = hex_value(value.as_bytes()[1]).ok_or(TraceError::InvalidTraceparent)?;
        Ok(Self((high << 4) | low))
    }

    /// Creates flags for an SDK adapter.
    #[must_use]
    pub const fn new(value: u8) -> Self {
        Self(value)
    }

    /// Returns the raw flags byte.
    #[must_use]
    pub const fn value(self) -> u8 {
        self.0
    }

    fn as_hex(self) -> String {
        format!("{:02x}", self.0)
    }
}

/// Correlation context propagated across one call's subsystem boundaries.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TraceContext {
    call_id: CallId,
    trace_id: TraceId,
    span_id: SpanId,
    parent_span_id: Option<SpanId>,
    flags: TraceFlags,
}

impl TraceContext {
    /// Creates a deterministic root context for a newly allocated call.
    #[must_use]
    pub fn from_sequence(call_id: CallId, sequence: u64) -> Self {
        Self {
            call_id,
            trace_id: TraceId::from_sequence(sequence),
            span_id: SpanId::from_sequence(sequence),
            parent_span_id: None,
            flags: TraceFlags::new(1),
        }
    }

    /// Creates a root context from externally supplied W3C-compatible IDs.
    #[must_use]
    pub fn root(call_id: CallId, trace_id: TraceId, span_id: SpanId, flags: TraceFlags) -> Self {
        Self {
            call_id,
            trace_id,
            span_id,
            parent_span_id: None,
            flags,
        }
    }

    /// Parses a W3C `traceparent` received from an upstream service.
    ///
    /// The parsed span is treated as the current remote parent. Callers should
    /// use [`Self::child`] before starting local work.
    pub fn from_traceparent(call_id: CallId, value: &str) -> Result<Self, TraceError> {
        if value.len() != TRACEPARENT_BYTES || !value.is_ascii() {
            return Err(TraceError::InvalidTraceparent);
        }
        let bytes = value.as_bytes();
        if bytes[2] != b'-' || bytes[35] != b'-' || bytes[52] != b'-' {
            return Err(TraceError::InvalidTraceparent);
        }
        if &value[..2] != "00" || value[53..].contains('-') {
            return Err(TraceError::InvalidTraceparent);
        }
        let trace_id =
            TraceId::from_hex(&value[3..35]).map_err(|_| TraceError::InvalidTraceparent)?;
        let span_id =
            SpanId::from_hex(&value[36..52]).map_err(|_| TraceError::InvalidTraceparent)?;
        let flags = TraceFlags::from_hex(&value[53..55])?;
        Ok(Self {
            call_id,
            trace_id,
            span_id,
            parent_span_id: None,
            flags,
        })
    }

    /// Returns the application-owned call identifier carried as correlation metadata.
    #[must_use]
    pub fn call_id(&self) -> &CallId {
        &self.call_id
    }

    /// Returns the trace ID shared by all child spans.
    #[must_use]
    pub fn trace_id(&self) -> &TraceId {
        &self.trace_id
    }

    /// Returns this span's ID.
    #[must_use]
    pub fn span_id(&self) -> &SpanId {
        &self.span_id
    }

    /// Returns the parent span when this context was derived with [`Self::child`].
    #[must_use]
    pub fn parent_span_id(&self) -> Option<&SpanId> {
        self.parent_span_id.as_ref()
    }

    /// Returns the W3C trace flags.
    #[must_use]
    pub const fn flags(&self) -> TraceFlags {
        self.flags
    }

    /// Serializes this context as a W3C `traceparent` value.
    #[must_use]
    pub fn traceparent(&self) -> String {
        format!(
            "00-{}-{}-{}",
            self.trace_id.as_hex(),
            self.span_id.as_hex(),
            self.flags.as_hex()
        )
    }

    /// Creates a child context while retaining the call and trace IDs.
    pub fn child(&self, span_id: SpanId) -> Result<Self, TraceError> {
        if span_id == self.span_id {
            return Err(TraceError::DuplicateSpan);
        }
        Ok(Self {
            call_id: self.call_id.clone(),
            trace_id: self.trace_id.clone(),
            span_id,
            parent_span_id: Some(self.span_id.clone()),
            flags: self.flags,
        })
    }

    /// Starts a bounded named span from this context.
    pub fn span(&self, operation: impl Into<String>) -> Result<TraceSpan, TraceError> {
        TraceSpan::new(self.clone(), operation)
    }
}

/// A bounded operation name paired with a propagated [`TraceContext`].
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TraceSpan {
    context: TraceContext,
    operation: String,
}

impl TraceSpan {
    /// Creates a span after validating its bounded operation name.
    pub fn new(context: TraceContext, operation: impl Into<String>) -> Result<Self, TraceError> {
        let operation = operation.into();
        if operation.is_empty()
            || operation.len() > MAX_TRACE_OPERATION_BYTES
            || !operation.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(TraceError::InvalidOperation);
        }
        Ok(Self { context, operation })
    }

    /// Returns the propagated context, including the stable call ID.
    #[must_use]
    pub fn context(&self) -> &TraceContext {
        &self.context
    }

    /// Returns the bounded operation name.
    #[must_use]
    pub fn operation(&self) -> &str {
        &self.operation
    }

    /// Creates a child span with a new span ID and operation name.
    pub fn child(&self, span_id: SpanId, operation: impl Into<String>) -> Result<Self, TraceError> {
        Self::new(self.context.child(span_id)?, operation)
    }
}

fn decode_hex<const N: usize>(value: &str) -> Option<[u8; N]> {
    if value.len() != N * 2 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut output = [0_u8; N];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        output[index] = (hex_value(chunk[0])? << 4) | hex_value(chunk[1])?;
    }
    Some(output)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[usize::from(byte >> 4)] as char);
        output.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call() -> CallId {
        CallId::from_sequence(7)
    }

    #[test]
    fn traceparent_round_trips_and_preserves_call_correlation() {
        let context = TraceContext::from_traceparent(
            call(),
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        )
        .unwrap();
        assert_eq!(context.call_id().as_str(), "call_7");
        assert_eq!(
            context.traceparent(),
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
        );

        let child = context.child(SpanId::from_sequence(8)).unwrap();
        assert_eq!(child.call_id(), context.call_id());
        assert_eq!(child.trace_id(), context.trace_id());
        assert_eq!(child.parent_span_id(), Some(context.span_id()));
        assert_ne!(child.traceparent(), context.traceparent());
    }

    #[test]
    fn malformed_or_zero_trace_data_is_rejected() {
        assert!(matches!(
            TraceId::from_hex("00"),
            Err(TraceError::InvalidTraceId)
        ));
        assert!(matches!(
            SpanId::from_hex("0000000000000000"),
            Err(TraceError::ZeroIdentifier)
        ));
        assert!(matches!(
            TraceContext::from_traceparent(
                call(),
                "00-00000000000000000000000000000000-00f067aa0ba902b7-01"
            ),
            Err(TraceError::InvalidTraceparent)
        ));
        assert!(matches!(
            TraceContext::from_traceparent(
                call(),
                "01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
            ),
            Err(TraceError::InvalidTraceparent)
        ));
    }

    #[test]
    fn span_operation_is_bounded_and_child_metadata_is_stable() {
        let context = TraceContext::from_sequence(call(), 1);
        let span = context.span("sip.receive").unwrap();
        assert_eq!(span.operation(), "sip.receive");
        assert_eq!(span.context().call_id(), context.call_id());
        assert!(matches!(
            context.span(""),
            Err(TraceError::InvalidOperation)
        ));
        assert!(matches!(
            context.span("x\n"),
            Err(TraceError::InvalidOperation)
        ));
        assert!(matches!(
            context.span("x".repeat(MAX_TRACE_OPERATION_BYTES + 1)),
            Err(TraceError::InvalidOperation)
        ));
        assert!(matches!(
            context.child(context.span_id().clone()),
            Err(TraceError::DuplicateSpan)
        ));
    }
}
