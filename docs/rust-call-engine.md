# Rust call engine

`call-engine` is the provider-neutral orchestration boundary for the Rust SIP
core. It composes the parser-facing SIP types, transaction state machines,
tag-qualified dialogs, SDP/audio negotiation, and the bounded call registry.
It does not own sockets, an async runtime, provider credentials, or Asterisk
configuration.

## Message flow

The transport adapter passes parsed requests and responses to `CallEngine` with
the source/destination address, monotonic time, and transport reliability. The
engine returns ordered `SendAction` values and lifecycle events. The adapter is
responsible for serialization and I/O.

Inbound INVITEs create one bounded call and UAS dialog, immediately return
`100 Trying`, and wait for `respond_to_invite` to produce provisional or final
responses. Successful 2xx responses are retained for retransmission until the
matching ACK arrives; a retransmitted INVITE replays the same response rather
than creating another call. CANCEL is matched by `(Via branch, CSeq method)`
and the INVITE branch, returns `200 OK` to CANCEL, and terminates the INVITE
with `487 Request Terminated`.

Outbound `originate` creates a UAC dialog and client transaction. Provisional
responses update lifecycle state. A final response generates the appropriate
ACK: successful INVITE ACKs use a new branch, while non-2xx ACKs reuse the
INVITE transaction branch. Duplicate final responses replay ACK generation.

`poll` accepts deterministic monotonic time and emits transaction
retransmissions, successful UAS 2xx retransmissions, and timeout-driven call
failure/cleanup. The operation runs against a cloned engine state and commits
only on success, so event-queue and state-transition errors do not leave a
partially applied call operation.

## Retry-safe application commands

Use `CommandId` with `apply_idempotent_call_command` when a control-plane
caller may retry a command. A matching retry returns the original lifecycle
event without changing call state or adding a duplicate pending event. Reusing
an active key for another call or command returns `IdempotencyConflict` and
leaves the engine unchanged. The registry bounds retained keys with
`CallRegistryConfig::max_command_keys`; once a key is evicted, it may be reused
as a new operation.

Application-facing wrappers accept an `AuthenticatedPrincipal` created by an
outer authentication adapter with `from_verified_claims`. That handoff stores
only a bounded, non-secret principal ID and permission bits; bearer tokens,
passwords, signatures, and verification keys stay in the adapter. Authorized
snapshot/list/replay, originate, response, negotiation, command, idempotent
retry, and terminal-reclamation calls check permissions before call lookup or
idempotency-key lookup, so denied requests cannot probe state or replay events.
The existing unqualified methods remain trusted internal APIs for SIP-driven
engine work.

## Aggregate metrics

`CallEngine::metrics` returns bounded lifecycle counters and signaling gauges.
`EngineMetrics::prometheus` renders a label-free Prometheus text snapshot with
call starts, answers, failures, completions, active calls, event queue/history
depth, retained idempotency keys, active transactions/dialogs, final INVITE
retransmission state, and reliable provisional responses awaiting PRACK. The
snapshot intentionally contains no call IDs, SIP Call-IDs, phone numbers,
provider names, principal IDs, or credentials, preventing sensitive values and
unbounded per-call label cardinality from entering metrics. `CallRuntime::metrics`
exposes the same snapshot at the transport boundary.

## Health and readiness

`CallEngine::health` and `CallRuntime::health` expose a small, runtime-agnostic
health contract. `live` is true for a successfully constructed engine;
`ready` is true only while another retained call record, SIP transaction, and
pending lifecycle event can be admitted under the configured bounds. Terminal
calls continue to count against readiness until `reclaim_terminal_call` has
released their resources. `EngineHealth::prometheus` provides a label-free
snapshot suitable for a health/readiness adapter without exposing call or SIP
identifiers.

## Safety boundary

Engine and registry bounds reject zero-sized configurations and cap calls,
events, dialogs, routes, branches, and transactions. Malformed Via branches
and CSeq values are rejected before call creation. All protocol behavior in
this crate remains offline and provider-neutral; Asterisk routing remains the
fallback until production provider/runtime evidence and sanitized SIP/SDP/RTP
fixtures are available.
