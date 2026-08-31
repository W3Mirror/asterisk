# Rust SIP call runtime

`call-runtime::CallRuntime` is the blocking I/O adapter that connects the
provider-neutral `CallEngine` to the existing bounded UDP/TCP SIP transports.
It is deliberately small: transport framing and socket ownership live here,
while transaction timers, dialogs, call state, and response generation remain
in `call-engine`.

## Dispatch contract

- UDP reads are dispatched with `TransportReliability::Unreliable`, preserving
  SIP retransmission timers and returning each action to its selected address.
- Connected TCP reads are dispatched with `TransportReliability::Reliable` and
  may contain multiple framed SIP messages. Actions must target the connected
  peer, preventing accidental cross-connection delivery.
- `receive_once` and `poll` clone the engine before processing and only commit
  state after all generated actions are delivered. A malformed batch or a
  delivery error therefore cannot leave a partially applied engine state.
- `begin_drain`, `resume`, and `is_draining` delegate the engine's graceful
  restart admission state. During a drain, outbound origination is rejected
  with `EngineError::Draining`; new inbound initial INVITEs receive a stateless
  `503 Service Unavailable`, while existing dialogs and transactions continue.
  Runtime health/readiness exposes the same drain state and reports `ready` as
  false until `resume` is called.
- UDP and connected TCP runtimes default to the PR12 source policy's
  default-allow behavior for backward compatibility. Use
  `udp_with_source_policy`, `tcp_with_source_policy`, or
  `with_source_policy` to require an observed peer address to match configured
  CIDRs. Denied peers fail before `CallEngine` dispatch, and deny rules retain
  precedence over allows.

Application retries can use `apply_idempotent_call_command` with a stable
`CommandId`. The runtime delivers the original lifecycle event again without
reapplying the transition; a conflicting reuse is rejected atomically by the
engine. The bounded key store is configured through the engine's
`CallRegistryConfig`.

Control-plane callers can use the `*_authorized` runtime wrappers with an
`AuthenticatedPrincipal` produced by an outer authentication adapter. The
runtime retains no bearer token, password, signature, or verification key—only
the bounded non-secret principal ID and permission bits are handed to the
engine. Authorization runs before call lookup and idempotency-key lookup, and
denials preserve call state, lifecycle queues, and transport output. Their
bounded audit record is intentionally committed even though no wire action is
produced, so rejected control-plane attempts remain observable. The
unqualified methods remain available for trusted internal/SIP dispatch.

Authorized control-plane operations append bounded, credential-free audit
records. A record contains the verified principal ID, application call ID,
operation, and stable outcome code; it excludes SIP Call-IDs, phone numbers,
credentials, and raw request bodies. A caller with `calls:read` (or
`calls:admin`) can drain the records with `drain_audit_records`. The oldest
record is evicted at the configured event bound, and queue depth is exposed as
an aggregate metric without labels.

The adapter has no async-runtime or provider-specific dependency. An
application can call it from its own event loop, wrap it in an async worker, or
keep Asterisk as the configured route while interoperability evidence is
collected.
