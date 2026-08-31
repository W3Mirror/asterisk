# Rust media load and reclamation smoke

The `load-smoke` crate includes a bounded media-only harness for repeatable RTP
and AI-media capacity checks without provider access, sockets, sleeps, or live
traffic. It complements the existing signaling INVITE/CANCEL reclamation run.

Each simulated stream uses a real `MediaSession` and exercises:

- serialized PCMU RTP ingress and full RTP validation/accounting;
- caller-driven fixed-delay jitter retention and playout;
- decoded AI-facing queue saturation with observable drop-oldest backpressure;
- AI-originated PCM queueing and outbound RTP serialization;
- per-batch media-session destruction and capacity reuse; and
- best-effort Linux resident-memory and open-file-descriptor observations.

The report records attempted/completed/failed streams, batch and concurrency
peaks, inbound/played/outbound packets, queue and jitter drops, queue depths,
an estimate of logically retained audio payload bytes, elapsed throughput, and
before/peak/after process observations. Deterministic correctness assertions
use logical counters and queue bounds; RSS and throughput are observations
because allocator and host scheduling behavior are environment-dependent.

Run the ordinary CI-sized smoke with:

```sh
cargo run -p load-smoke --locked -- media 64 8 32 4
```

The arguments are total streams, concurrent streams, packets per stream, and
per-direction AI queue capacity. The weekly scheduled workflow also runs:

```sh
cargo run -p load-smoke --locked -- media 4096 256 128 8
```

This is the first media-only load/reclamation tier. It does not claim the goal's
1,000/5,000/10,000 concurrent-call capacity matrix, UDP socket or WebSocket
throughput, combined signaling-plus-media load, CPU-per-call profiling, a
multi-hour soak, or a stable allocator-memory baseline. Those remain separate
acceptance work before Rust traffic can be enabled; Asterisk remains the
production fallback.
