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
   configured or learned.

The runtime deliberately has no async-runtime, TLS/DTLS, SRTP, provider, call
routing, or Asterisk dependency. DTLS-SRTP and live provider interoperability
remain separate evidence-gated slices; existing call routing continues to use
the Asterisk fallback.
