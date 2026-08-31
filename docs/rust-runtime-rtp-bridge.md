# Runtime caller/human RTP audio bridge

`HumanMediaBridgeRuntime` composes two `MediaUdpRuntime` instances with one
answered caller/human `BridgeRegistry` record. It is intentionally a blocking,
single-datagram boundary that an application event loop can drive in either
direction.

Before every socket read, the runtime verifies that:

- the bridge still exists in `HumanActive`;
- the retained caller call and leg match the attached caller media runtime;
- the active human call and leg match the attached human media runtime; and
- the opposite RTP destination is configured or learned.

An invalid state or stale endpoint binding returns before consuming the queued
UDP datagram. This means a SIP failure, BYE, explicit AI resume, or replacement
human leg stops the old media pair immediately and lets the retained caller
return to AI routing.

Accepted audio takes the bounded media path in both directions:

1. the source session authorizes, parses, accounts for, and decodes RTP;
2. one decoded frame crosses into the destination session's bounded outbound
   queue;
3. the destination session re-encodes the frame using its negotiated codec,
   payload type, SSRC, sequence, and timestamp state; and
4. the destination UDP runtime sends the new RTP datagram.

Both inbound and outbound `PushOutcome` values are returned so drop-oldest or
drop-newest backpressure is observable.

Validated RFC 4733 telephone-event packets also relay in both directions while
the exact bridge endpoints remain active. Every accepted packet, including end
retransmissions, is re-encoded with the destination leg's payload type, SSRC,
and sequence number while retaining the source marker bit and event fields.
The source session still deduplicates application notifications independently,
so relay reliability does not create duplicate start/end events for consumers.
Packets belonging to one relayed event use a stable destination RTP timestamp.

Run the focused verification with:

```sh
cargo test -p call-runtime --locked
cargo clippy -p call-runtime --all-targets --no-deps --locked -- -D warnings
```

This localhost composition does not yet advance the destination RTP timeline
from a DTMF event into subsequent audio. It is also not jitter playout, RTCP
relay, transcoding beyond the existing negotiated G.711 session behavior,
provider authentication, Asterisk/carrier interoperability, or load/soak
evidence. Production traffic remains on Asterisk until the later compatibility,
reliability, and rollback gates pass.
