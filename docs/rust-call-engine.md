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

## Safety boundary

Engine and registry bounds reject zero-sized configurations and cap calls,
events, dialogs, routes, branches, and transactions. Malformed Via branches
and CSeq values are rejected before call creation. All protocol behavior in
this crate remains offline and provider-neutral; Asterisk routing remains the
fallback until production provider/runtime evidence and sanitized SIP/SDP/RTP
fixtures are available.
