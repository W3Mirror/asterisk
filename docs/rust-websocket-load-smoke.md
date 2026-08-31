# Rust WebSocket media load and reclamation smoke

The `load-smoke` crate includes a bounded WebSocket-media harness for repeatable
transport, framing, backpressure, and capacity-reuse checks without provider
access, network sockets, sleeps, or live traffic. It complements the signaling
and RTP/media-only reclamation tiers.

Each simulated stream uses a real `MediaWebSocketTransport`,
`MediaWebSocketSession`, and `MediaSession` to exercise:

- masked peer `MEDIA_START` and binary PCMU frame parsing;
- WebSocket audio decoding and outbound RTP serialization;
- inbound RTP validation and unmasked WebSocket audio serialization;
- bounded pending-write queues, observable full-queue backpressure, lossless
  flush/retry behavior, and deliberately partial underlying writes;
- decoding every emitted frame at the simulated peer boundary; and
- per-batch transport/media destruction and capacity reuse.

The report records attempted/completed/failed streams, batch and concurrency
peaks, frame and RTP counts in both directions, write-backpressure events,
pending-write and media-queue peaks, final logical resources, elapsed frame
throughput, and best-effort Linux resident-memory/file-descriptor observations.
Deterministic assertions use protocol counters and queue bounds; RSS and
throughput remain environment-dependent observations.

Run the ordinary CI-sized smoke with:

```sh
cargo run -p load-smoke --locked -- websocket 64 8 32 4
```

The arguments are total streams, concurrent streams, frames per stream, and
per-stream media/pending-write frame capacity. The weekly scheduled workflow
also runs:

```sh
cargo run -p load-smoke --locked -- websocket 4096 256 128 8
```

This is an in-memory transport load/reclamation tier. It does not claim actual
TCP, TLS, HTTP upgrade, kernel-socket, provider, or Asterisk throughput; the
goal's 1,000/5,000/10,000 concurrent-call matrix; combined signaling/media
load; CPU-per-call profiling; a multi-hour soak; or a stable allocator-memory
baseline. Those remain separate acceptance work before Rust traffic can be
enabled; Asterisk remains the production fallback.
