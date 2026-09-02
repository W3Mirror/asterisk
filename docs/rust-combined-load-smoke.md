# Rust combined signaling and media load smoke

The `load-smoke` crate includes a bounded combined harness that retains each
synthetic SIP call while a paired real `MediaSession` performs bidirectional
RTP, jitter-playout, and AI-queue work. This covers cross-layer resource overlap
that the separate signaling-only and media-only harnesses intentionally do not.

For each fixed-size batch, the harness:

- creates unique inbound INVITEs and retains their server transactions;
- creates one bounded RTP/media session per registered call;
- processes ordered PCMU RTP ingress, fixed-delay playout, AI-facing queue
  backpressure, AI-originated PCM, and outbound RTP;
- cancels every call and checks the expected `200` and `487` responses;
- drains every media queue, reclaims every terminal call, and requires calls,
  transactions, media sessions, and retained logical payload bytes to return to
  zero before reusing capacity; and
- records elapsed throughput plus best-effort Linux resident-memory and
  open-file-descriptor observations.

Run the ordinary CI-sized smoke with:

```sh
cargo run -p load-smoke --locked -- combined 64 8 32 4
```

The arguments are total calls, concurrent call/media pairs, bidirectional RTP
packets per call, and per-direction AI queue capacity. The weekly scheduled
workflow also runs:

```sh
cargo run -p load-smoke --locked -- combined 4096 256 128 8
```

The report includes attempted/completed/failed calls, batch count, peak calls,
transactions and media sessions, packet totals, queue/jitter drops, queue
depths, peak logically retained payload bytes, exact final logical resources,
elapsed packet throughput, RSS, and file descriptors. Deterministic assertions
use logical counters and queue bounds; timing and process values are observations
because allocator and host scheduling behavior are environment-dependent.

This is deterministic in-memory composition evidence. The harness pairs the
call engine and media sessions explicitly; it does not claim a provider-driven
SDP/media attachment path, kernel-socket throughput, end-to-end audio quality,
CPU per call or the same 1,000/5,000/10,000 media concurrency levels. The
separate [mixed-lifecycle soak](rust-lifecycle-soak.md) adds repeated lifecycle
and bounded process-resource evidence, but not a stable allocator-memory
baseline. Provider/Asterisk
interoperability and production evidence remain mandatory before enabling Rust
traffic, and Asterisk remains the fallback.
