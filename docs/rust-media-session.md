# Rust media session

`media-core::MediaSession` is the provider-neutral boundary between a
negotiated G.711 RTP stream and an AI application. It owns one bounded
`RtpSession`, a bidirectional `AudioBridge`, and a bounded DTMF notification
queue. Socket ownership, WebSocket framing, persistence, and call routing stay
outside the crate.

## Receive path

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

## Recording

`AudioRecorder` is a separate non-blocking sink for decoded frames. It bounds
both retained frame count and samples per frame, records first/last RTP
timestamps and drop counts, and can serialize the retained mono PCM as a WAV
file. Persistence or object-storage upload should consume `wav()` outside the
RTP processing loop.

This slice remains offline and provider-neutral. It does not enable Rust media
traffic, alter Asterisk configuration, or claim live-provider interoperability.
