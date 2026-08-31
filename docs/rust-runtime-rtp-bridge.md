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

When a source leg enables fixed-delay jitter buffering, socket ingress returns
`AudioBuffered` without forwarding audio. The event loop separately calls
`playout_caller_once` or `playout_human_once` at the session's next deadline.
Those operations revalidate the exact active bridge endpoints, perform no
socket read on the source leg, and forward at most one due frame using the
destination leg's own RTP identity. See
[`rust-jitter-playout.md`](rust-jitter-playout.md).

Validated RFC 4733 telephone-event packets also relay in both directions while
the exact bridge endpoints remain active. Every accepted packet, including end
retransmissions, is re-encoded with the destination leg's payload type, SSRC,
and sequence number while retaining the source marker bit and event fields.
The source session still deduplicates application notifications independently,
so relay reliability does not create duplicate start/end events for consumers.
Packets belonging to one relayed event use a stable destination RTP timestamp.
Each direction retains a fixed wrapping offset between source and destination
RTP clocks. A packet is serialized at the mapped event timestamp without
moving the regular-media clock; when audio resumes, that clock synchronizes to
the later of the mapped source-audio timestamp and the mapped event timestamp
plus the largest validated event duration. This covers lost event-end packets,
keeps late retransmissions on the original timestamp, and never moves subsequent
audio backward. Source timestamp rollover uses wrapping RTP ordering.

RTCP terminates independently on each attached media leg because forwarded RTP
uses the destination leg's SSRC, sequence, and timestamp state. The bridge can
state-gate and account for one caller or human RTCP compound datagram without
copying it to the opposite peer. It can also generate an identity-correct
Receiver Report back to either peer using that same leg's accepted RTP loss,
highest extended sequence, jitter, and Sender Report timing. Bridge fail-back
or endpoint replacement rejects RTCP before consuming the stale socket.
After either bridge direction has emitted RTP, the same state gate can poll a
due Sender Report for that destination leg. Each report uses the leg's local
SSRC, RTP timestamp, packet/payload-octet counters, caller-supplied NTP words,
and a constant-size successful-send schedule. Fail-back rejects the poll before
the schedule or RTCP counters advance.

Run the focused verification with:

```sh
cargo test -p call-runtime --locked
cargo clippy -p call-runtime --all-targets --no-deps --locked -- -D warnings
```

This localhost composition now supports optional fixed-delay jitter playout,
but not adaptive delay or packet-loss concealment. It is not transcoding beyond
the existing negotiated G.711 session behavior, provider authentication,
Asterisk/carrier interoperability, or load/soak evidence. Production traffic
remains on Asterisk until the later compatibility, reliability, and rollback
gates pass.
