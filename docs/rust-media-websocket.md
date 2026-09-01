# Rust media WebSocket adapter

`media-websocket::MediaWebSocketSession` is a bounded, runtime-agnostic
adapter for the plain-text mode of Asterisk `chan_websocket`. An HTTP server
or WebSocket runtime owns the listener, TLS, upgrade handshake, socket reads,
and writes; this crate receives bytes after the upgrade and returns encoded
bytes for the runtime to send.

## Supported protocol slice

- RFC 6455 frame decoding and encoding with endpoint-specific masking rules;
- bounded frame/message sizes and fragmentation depth;
- UTF-8 text, binary media, ping/pong, and validated close frames;
- the plain-text `MEDIA_START`, `ANSWER`, `HANGUP`, buffering, pause/continue,
  queue-drained, and media-direction controls represented by the public
  `MediaCommand` enum;
- raw PCMU/PCMA binary media, split into the negotiated
  `optimal_frame_size` and bridged through `media-core::MediaSession`.

The adapter keeps partial G.711 media between binary messages, so callers can
send arbitrary bounded chunks and still produce complete AI/RTP bridge frames.
AI-bound audio is peeked before encoding and removed only after successful
WebSocket serialization. Queue capacity and drop outcomes remain owned by
`MediaSession`.

## Deliberate boundaries

This slice does not own a socket or HTTP upgrade, and it does not enable Rust
traffic in the Asterisk deployment. JSON `chan_websocket` controls, channel
variable metadata, provider-specific WebSocket handshakes, live
interoperability, fuzzing, load/soak, and production/telephony evidence remain
follow-up work. Existing call routing therefore continues to use the Asterisk
path as the rollback and fallback route.
