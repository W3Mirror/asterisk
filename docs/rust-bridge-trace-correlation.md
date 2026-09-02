# Rust bridge and media trace correlation

`call-bridge::BridgeRegistry` retains one bounded `call_core::TraceContext`
for the lifetime of a caller/AI or caller/human bridge. The context belongs to
the original application `CallId`; it is not replaced when a human leg is
connected, fails, or is released.

## Propagation contract

- `BridgeSnapshot` and every `BridgeEvent` carry the bridge's exact trace
  context, including terminal events emitted before explicit reclamation.
- `BridgeRegistry::create_ai_with_trace_context` accepts an upstream context
  and rejects a context whose application call does not match the caller
  endpoint. Rejection is atomic and does not consume bridge or event capacity.
- The existing `create_ai` API remains deterministic for offline replay and
  local tests by creating a bounded root context from its allocated bridge
  sequence.
- `HumanMediaBridgeRuntime` copies the context from the active snapshot and
  exposes it through `trace_context()`. Media forwarding therefore has the
  same correlation metadata as bridge lifecycle events without exposing SIP
  Call-IDs, RTP addresses, SSRCs, or arbitrary baggage.

The context is metadata only: it does not authorize a bridge, alter endpoint
ownership, or enable Rust production traffic. Bridge state and media endpoint
checks remain the source of truth for routing and fail-back.

## Verification

The focused `call-bridge` tests verify supplied-context preservation on create,
snapshot, and human-leg events, plus atomic rejection of a mismatched call
context. The focused `call-runtime` media test verifies that an attached media
bridge retains the same context as its active bridge snapshot. The complete
workspace test and lint suites exercise these tests on every hosted pull
request and on pushes to `aistack/main`.

Provider credentials, sanitized provider/Asterisk captures, live calls,
deployment, and Rust traffic enablement remain later gates; Asterisk remains
the fallback.
