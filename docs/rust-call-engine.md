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

When a transport or process boundary becomes unusable, `fail_active_calls`
transitions every non-terminal call through `Failed` and `Ended` (or completes
an already-ending call), removes all owned SIP dialogs, transactions, and
retransmission state, and returns the terminal lifecycle events without
generating wire actions. Terminal call records remain bounded and retained
until `reclaim_terminal_call` is called, allowing post-call consumers to export
state before releasing the registry slot. The operation is transactional and
idempotent.

For a controlled process replacement, `prepare_restart_handoff` combines that
terminal cleanup with admission drain. It returns a bounded report containing
the application call IDs that were active at handoff and their ordered
terminal lifecycle events, leaves the engine draining, and emits no wire
actions. Repeating the handoff is safe and returns no duplicate terminal
events. A supervisor can persist the report and exported diagnostics before
dropping the old engine; `resume` is available when a planned restart is
cancelled.

## Graceful drain and restart

`begin_drain` stops new call admission without interrupting existing dialogs or
transactions. While draining, outbound `originate` returns the stable
`EngineError::Draining` error, and a new inbound initial INVITE receives a
stateless `503 Service Unavailable` without creating a call, dialog, or
transaction. Retransmissions and in-dialog requests for calls admitted before
the drain continue normally. `resume` reopens admission. The drain flag is
included in `EngineHealth`, `EngineMetrics`, and their label-free Prometheus
snapshots; readiness is false for the entire drain window so a deployment can
stop traffic before handing the endpoint to Asterisk or a replacement process.

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

Authorized command, origination, response, media-negotiation, and terminal
reclamation paths append a bounded `AuditRecord` containing only the verified
principal ID, application call ID, operation, and stable outcome code. Audit
records never contain SIP Call-IDs, phone numbers, credentials, or raw request
bodies. Consumers with `calls:read` (or `calls:admin`) can drain the records
through `CallEngine::drain_audit_records`; the oldest record is evicted at the
configured event bound so audit traffic cannot create an unbounded queue or
block the media path.

## Aggregate metrics

`CallEngine::metrics` returns bounded lifecycle counters and signaling gauges.
`EngineMetrics::prometheus` renders a label-free Prometheus text snapshot with
call starts, answers, failures, completions, active calls, event queue/history
depth, retained idempotency keys, active transactions/dialogs, final INVITE
retransmission state, reliable provisional responses awaiting PRACK, and
bounded audit queue depth. The
snapshot intentionally contains no call IDs, SIP Call-IDs, phone numbers,
provider names, principal IDs, or credentials, preventing sensitive values and
unbounded per-call label cardinality from entering metrics. The aggregate
`engine_draining` gauge makes admission state visible without labels.
`CallRuntime::metrics` exposes the same snapshot at the transport boundary.

## Health and readiness

`CallEngine::health` and `CallRuntime::health` expose a small, runtime-agnostic
health contract. `live` is true for a successfully constructed engine;
`ready` is true only while another retained call record, SIP transaction, and
pending lifecycle event can be admitted under the configured bounds and the
engine is not draining. Terminal calls continue to count against readiness
until `reclaim_terminal_call` has released their resources. `begin_drain` and
`resume` are idempotent control-plane operations; query `is_draining` before a
handoff when needed. `EngineHealth::prometheus` provides a label-free snapshot
suited to a health/readiness adapter without exposing call or SIP identifiers.

## Safety boundary

Engine and registry bounds reject zero-sized configurations and cap calls,
events, dialogs, routes, branches, and transactions. Malformed Via branches
and CSeq values are rejected before call creation. All protocol behavior in
this crate remains offline and provider-neutral; Asterisk routing remains the
fallback until production provider/runtime evidence and sanitized SIP/SDP/RTP
fixtures are available.
