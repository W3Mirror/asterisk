# Rust media WebSocket adapter

`media-websocket::MediaWebSocketSession` is a bounded, runtime-agnostic
adapter for the plain-text mode of Asterisk `chan_websocket`. The companion
`MediaWebSocketTransport<S, M>` drives that adapter over an already-upgraded
`Read + Write` stream. It retains incomplete input, bounds the outbound frame
queue, handles partial writes, and automatically queues pong and close replies.
An HTTP server or WebSocket runtime still owns the listener, TLS, and upgrade
handshake; this crate does not assume a particular async runtime.

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

The stream driver provides the network boundary without weakening those
guarantees:

- `read_once` reads one bounded chunk and processes every complete frame;
- incomplete frames remain buffered up to `max_buffered_bytes`;
- `queue_audio` checks output capacity before consuming the AI queue;
- `flush` retains partially written frames and reports zero-byte writes; and
- client-role connections require a fresh `MaskKeySource` for every outbound
  frame, with `OsRandomMaskKeySource` available for the Linux deployment
  target, rather than falling back to predictable keys.

## Deliberate boundaries

This slice does not perform the HTTP upgrade or TLS, and it does not enable
Rust traffic in the Asterisk deployment. JSON `chan_websocket` controls,
channel-variable metadata, provider-specific WebSocket handshakes, live
interoperability, fuzzing, load/soak, and production/telephony evidence remain
follow-up work. Existing call routing therefore continues to use the Asterisk
path as the rollback and fallback route.
