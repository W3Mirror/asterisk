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

The adapter has no async-runtime or provider-specific dependency. An
application can call it from its own event loop, wrap it in an async worker, or
keep Asterisk as the configured route while interoperability evidence is
collected.
