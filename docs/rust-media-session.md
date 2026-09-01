# Rust media session

`media-core::MediaSession` is the provider-neutral boundary between a
negotiated G.711 RTP/RTCP stream and an AI application. It owns one bounded
`RtpSession`, one bounded `RtcpSession`, a bidirectional `AudioBridge`, and a
bounded DTMF notification queue. Socket ownership, WebSocket framing,
persistence, and call routing stay outside the crate.

The bounded plain-text Asterisk `chan_websocket` adapter is documented in
[`rust-media-websocket.md`](rust-media-websocket.md). It supplies raw PCMU/PCMA
chunks to and from this session without changing the session's RTP/RTCP or
queue ownership.

## Receive path

For socket-backed ingress, call `receive_rtp_from` with the observed
`SocketAddr`. The configured `sip-security::SourceIpPolicy` is evaluated before
RTP parsing, so denied peers cannot change parse counters, SSRC/sequence state,
queues, DTMF state, or quality metrics. `new` remains default-allow for callers
that already enforce source policy at a lower transport boundary; use
`new_with_source_policy` or `with_source_policy` to attach an explicit policy.

After source authorization:

1. Parse and size-check the RTP packet.
2. Accept the negotiated audio payload or the negotiated RFC 4733
   `telephone-event` payload while sharing SSRC, sequence, loss, and jitter
   accounting.
3. Decode PCMU or PCMA audio to bounded PCM samples and enqueue an
   `AudioFrame` for the AI adapter.
4. Deduplicate DTMF start/end packets and enqueue lifecycle notifications with
   an explicit drop counter when the application bound is full.

## Send path

The adapter pushes decoded AI frames into the bounded return queue and calls
`next_audio_rtp` to serialize one packet at a time. Codec, sample-rate, frame
size, and RTP packet bounds are checked before a frame is removed from the
queue. `send_dtmf` emits a telephone-event packet with an explicit timestamp
increment so retransmissions can reuse the event timestamp.

## RTCP path

Call `receive_rtcp` or `receive_rtcp_from` for remote RTCP datagrams and
`send_rtcp` for locally generated reports. The RTCP session shares the RTP
packet bound, expected remote SSRC, and observed-source policy. Its statistics
are exposed under `MediaSessionStats::rtcp`, including report-derived loss,
jitter, and matching Sender Report/Reception Report RTT estimates.

## Recording

`AudioRecorder` is a separate non-blocking sink for decoded frames. It bounds
both retained frame count and samples per frame, records first/last RTP
timestamps and drop counts, and can serialize the retained mono PCM as a WAV
file. Persistence or object-storage upload should consume `wav()` outside the
RTP processing loop.

This slice remains offline and provider-neutral. It does not enable Rust media
traffic, alter Asterisk configuration, or claim live-provider interoperability.
