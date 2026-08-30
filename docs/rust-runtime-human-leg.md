# Runtime human-leg orchestration

`CallRuntime` can attach a bounded `BridgeRegistry` and originate a human SIP
leg through the same UDP or TCP transport that drives `CallEngine`.

`originate_human_leg` prepares the outbound INVITE transaction and the
`ConnectingHuman` bridge transition on cloned state. It writes the INVITE only
after both operations succeed, then commits the call engine and bridge registry
together. A bridge validation or transport error leaves both state machines
unchanged.

Subsequent runtime operations synchronize human-call lifecycle with routing:

- a successful final INVITE response sends ACK and changes the bridge to
  `HumanActive`;
- a non-success final response or transaction timeout restores `AiActive`;
- a remote human BYE receives `200 OK`, ends that call, and restores `AiActive`;
- provisional responses leave the bridge in `ConnectingHuman`, so AI remains
  active while the destination rings.

Bridge events are drained into `RuntimeOutput` in order, matching the existing
call lifecycle-event delivery model. This prevents a correctly consumed runtime
from filling the registry's bounded pending-event queue.

Run the focused verification with:

```sh
cargo test -p call-runtime --locked
cargo clippy -p call-runtime --all-targets --no-deps --locked -- -D warnings
```

This composition does not yet forward RTP between caller and human media
sessions, generate provider-specific SIP identities, authenticate to a
provider, or prove interoperability with Asterisk or a real carrier. Production
traffic remains on Asterisk until those later gates and rollback evidence pass.
