# Rust trace correlation

The Rust call stack carries a bounded `call_core::TraceContext` with every
retained call. It is deliberately an SDK-neutral boundary: an application can
adapt it to OpenTelemetry without making the SIP engine depend on a particular
runtime or exporter.

## Context contract

- `TraceId` is a validated, non-zero 16-byte W3C trace ID.
- `SpanId` is a validated, non-zero 8-byte W3C span ID.
- `TraceContext::traceparent()` emits the canonical version-`00` W3C value.
- `TraceContext::from_traceparent()` accepts only the bounded 55-byte
  version-`00` form and rejects malformed or zero identifiers.
- Every context retains the application-owned `call_id` as correlation
  metadata. SIP Call-ID, dialog tags, addresses, credentials, and baggage are
  not copied into the context.
- `TraceContext::child()` preserves the call and trace IDs while recording the
  parent span. `TraceSpan` adds a printable operation name capped at 64 bytes.

The registry creates deterministic roots for offline replay and local tests.
Production adapters should supply an SDK-generated root or an upstream
`traceparent`, then create a child span for local work.

## Propagation points

`LifecycleEvent`, `CallSnapshot`, `AuditRecord`, and `CallDiagnostics` carry the
same context for a retained call. `CallEngine::trace_context()` and
`CallRuntime::trace_context()` expose it to media, AI-gateway, STT, LLM, and TTS
adapters without exposing protocol identities. `originate_with_trace_context()`
allows a trusted API boundary to register a caller-supplied context while the
existing `originate()` path keeps deterministic root creation.

The context has no dynamic collection or arbitrary baggage field. Operation
names and identifiers are validated before allocation, and terminal call
reclamation removes the retained context together with the call record.

## Test coverage

`call-core` tests cover traceparent round trips, child-parent relationships,
malformed/zero identifiers, operation bounds, and duplicate span rejection.
`call-api` tests verify that one context is preserved across lifecycle events,
snapshots, and audit records. `call-engine` tests verify propagation from an
upstream context through emitted events and redaction-safe diagnostics.
