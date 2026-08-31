# Authenticated Rust bridge controls

`call-runtime` now exposes authorization-aware wrappers for the application
operations that control an AI-to-human bridge. The existing lower-level
`BridgeRegistry` remains a provider-neutral state machine; runtime wrappers
clone its bounded state, apply one transition, drain the ordered bridge events,
and commit only after validation succeeds.

## Authorization boundary

- Starting a human second leg requires `calls:transfer` before bridge lookup,
  call allocation, address use, or SIP delivery. Transfer permission covers
  the underlying second-leg origination because it is an implementation detail
  of the transfer operation.
- Completing a pending leg, failing a leg, or restoring AI routing requires
  `calls:transfer`.
- Ending bridge forwarding requires `calls:hangup`.
- Authorization failures are returned as stable call-API permission errors and
  leave engine, bridge, event, and transport state unchanged.

Normal SIP response processing remains the source of truth for automatic
`HumanConnected` and `HumanFailed` transitions. The explicit complete/fail,
resume, and end wrappers are for an already-authenticated supervisor or
control-plane adapter that needs to apply a bounded bridge transition.

## Fallback and test contract

This slice does not enable Rust traffic or change provider configuration.
Asterisk remains the routing fallback. Focused runtime authorization and
transactionality tests ship with the implementation and are exercised by the
complete hosted workspace suite on pull-request events and pushes to
`aistack/main`; scheduled capacity, soak, and live-provider evidence remain
separate gates.
