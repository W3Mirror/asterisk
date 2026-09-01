# Rust RTP/RTCP UDP runtime

`media-runtime::MediaUdpRuntime` is the blocking, provider-neutral network
boundary for one `media-core::MediaSession`. It owns separate UDP sockets for
RTP audio/telephone events and RTCP reports, while parsing, source policy,
SSRC validation, queues, DTMF, and quality metrics remain in `MediaSession`.

## Bounds and source handling

`MediaUdpRuntimeConfig::max_datagram_bytes` must be no larger than the media
session's RTP packet bound. The runtime allocates one reusable receive buffer
at that bound plus one byte, allowing an oversized UDP datagram to be rejected
before it reaches protocol parsing. A source endpoint is learned only after a
datagram passes `MediaSession` validation; denied or malformed packets cannot
redirect subsequent replies. Set an explicit `sip-security::SourceIpPolicy`
for internet-facing sockets.

RTP and RTCP destinations may also be supplied explicitly with
`set_remote_rtp` and `set_remote_rtcp`. Endpoint learning is enabled by
default for symmetric NAT behavior and can be disabled when the caller owns
endpoint selection.

## Drive loop

An application can register `rtp_socket()` and `rtcp_socket()` with its own
event loop, or set them non-blocking through the mutable socket accessors:

1. call `receive_rtp` or `receive_rtcp` with the caller's monotonic arrival
   timestamp;
2. drain `MediaSession`'s AI and DTMF outputs;
3. call `send_audio`, `send_dtmf`, or `send_rtcp` when a destination is
   configured or learned;
4. poll `send_sender_report_if_due` with the current monotonic time and its
   correlated NTP seconds/fraction words.

`send_receiver_report` checks the leg's RTCP destination before advancing its
report interval, builds a Receiver Report from that leg's accepted RTP/RTCP
state, and sends it on the RTCP socket. It fails explicitly until both an RTCP
destination and a valid remote RTP source exist.

`MediaUdpRuntimeConfig::sender_report_interval` defaults to five seconds and
must be non-zero. A Sender Report becomes due after the first serialized RTP
packet, then only after each successful interval. Missing RTP send state or an
interval that is not due returns `Ok(None)`. Missing endpoints, serialization
errors, and socket failures do not advance the schedule, so the same instant
can be retried safely. The runtime does not spawn a timer or read wall-clock
time; the owning event loop supplies both clocks.

The runtime deliberately has no async-runtime, TLS/DTLS, SRTP, provider, call
routing, or Asterisk dependency. DTLS-SRTP and live provider interoperability
remain separate evidence-gated slices; existing call routing continues to use
the Asterisk fallback.
