# Rust property testing

The `property-tests` workspace crate exercises cross-layer invariants with
`proptest`. It runs as part of the ordinary locked workspace suite on every
pull request and every push to `aistack/main`.

The current properties cover:

- SIP parse/serialize idempotence with normalized content length;
- SDP, RTP, RTCP, and DTMF serialization round trips;
- RTP sequence-number and timestamp rollover without manufactured loss;
- duplicate DTMF suppression and duplicate INVITE call identity;
- bounded media queues under both drop policies;
- SIP client timer ordering and reliable-transport termination;
- dialog retransmission sequence invariants;
- bounded call creation, terminal reclamation, and capacity reuse;
- bridge invalid-transition atomicity, stable caller ownership, human-leg
  failure recovery, endpoint release, and terminal capacity reuse.

Run the ordinary property suite with:

```text
cargo test -p property-tests --locked
```

The weekly `Rust quality` schedule repeats it with `PROPTEST_CASES=4096` after
the complete workspace suite. Developers can use the same environment variable
for a deeper local run.

Proptest's default source-parallel failure persistence writes minimized seeds
under `crates/property-tests/proptest-regressions/`. Any generated regression
file must be committed with its fix so all later workspace runs replay the
counterexample. The directory is intentionally tracked and not ignored.
