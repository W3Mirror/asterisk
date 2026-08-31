# Rust call diagnostics

`call-engine` exposes a bounded diagnostic view for each retained call through
`CallEngine::diagnostics` and deterministic `list_diagnostics`. `CallRuntime`
delegates the same view at the transport boundary. Read-authorized variants
perform the permission check before looking up a call, so a caller without
`calls:read` cannot probe whether an identifier exists.

The view combines the application lifecycle state with small, useful protocol
signals:

- dialog role/state, local and remote sequence numbers, route count, and remote
  tag presence;
- negotiated audio payload types, direction, and remote port; and
- client/server transaction counts, pending final-response retransmissions,
  reliable-provisional state, and Digest retry count.

The view deliberately omits SIP Call-IDs, tags, request URIs, network
addresses, credentials, and raw message bodies. It is therefore suitable for
post-call export and operational inspection without leaking secrets or
creating unbounded metric-label cardinality. Terminal calls remain available
until `reclaim_terminal_call` is called; reclamation removes the diagnostic
record along with the underlying signaling resources.

Diagnostics are a local/offline contract. They do not imply provider
interoperability, production deployment, or permission to enable Rust traffic;
Asterisk remains the fallback until those later evidence gates pass.
