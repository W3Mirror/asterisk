# Rust call bridge state

`call-bridge::BridgeRegistry` is the bounded, provider-neutral control-plane
model for switching one stable inbound caller between an AI media stream and a
server-originated human SIP/PSTN leg. It does not own SIP transactions, RTP
sessions, sockets, or provider routing. Those components compose the bridge
state and perform the requested signaling/media actions.

Each retained bridge owns these identities exclusively:

- the original inbound `CallId` and `LegId`;
- the retained AI `StreamId` used for initial routing and fail-back;
- at most one pending or active human `CallId` and `LegId`.

The inbound identities never change during destination switches. Endpoint
ownership prevents the same call, leg, or AI stream from appearing in two
retained bridges.

## State transitions

```text
AiActive
    |
    | begin_human (AI remains active)
    v
ConnectingHuman -- fail_human --> AiActive
    |
    | complete_human
    v
HumanActive ----- fail_human --> AiActive
    |
    | resume_ai
    v
AiActive

Any nonterminal state -- end --> Ended -- remove_terminal --> reclaimed
```

`fail_human` covers both an outbound setup failure and failure of an active
human leg. In either case the human identities are released and the retained
AI stream becomes authoritative again. `end` clears human identities but keeps
the terminal record available for inspection until explicit reclamation.

## Bounds and atomicity

The registry bounds both retained bridge records and undelivered lifecycle
events. Every operation reserves event capacity before changing bridge state;
invalid transitions, duplicate endpoint ownership, and event backpressure
leave the registry unchanged. Terminal reclamation releases the caller, leg,
stream, and bridge-capacity slot for deterministic reuse.

This foundation does not yet originate the human SIP transaction or forward
media between two RTP sessions. Those runtime integrations must consume this
state model in later slices and retain Asterisk as the production fallback
until provider and real-call evidence satisfies the goal's traffic gates.
