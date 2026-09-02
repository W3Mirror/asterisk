# Rust mixed-lifecycle soak

The `load-smoke` crate includes a same-process soak harness for repeated call
lifecycle and resource-reclamation evidence. Every cycle creates a bounded mix
of calls, then:

- answers one third, accepts ACK, processes bidirectional RTP/media work, and
  disconnects them with BYE;
- rejects one third and accepts their non-success ACKs;
- cancels one third before answer; and
- reclaims every terminal call before the next cycle reuses capacity.

After every cycle, the harness requires exact zero final calls, SIP
transactions, dialogs, media sessions, queues, and logically retained media
payload. The standalone executable also requires Linux file-descriptor and
thread counts to match their initial values. After configurable warmup cycles,
it fails when the observed process RSS range exceeds its configured bound.

Run the short ordinary-CI tier with:

```sh
cargo run -p load-smoke --locked -- soak 8 0 12 8 4 2 16777216
```

The arguments are minimum cycles, minimum seconds, calls per cycle, packets per
answered call, queue capacity, warmup cycles, and maximum post-warmup RSS range
in bytes. Both cycle and duration minima must be satisfied.

The dedicated scheduled job runs the executable script for at least two hours:

```sh
tests/rust-lifecycle-soak/run.sh
```

Manual workflow dispatches run that job only when the `lifecycle_soak` input is
selected. The script accepts the same bounds with minimum seconds first, so a
short standalone probe can be run as
`tests/rust-lifecycle-soak/run.sh 5 8 12 8 4 2 67108864`.

Unit tests disable only process-wide descriptor/thread equality because Rust's
parallel test runner can change those values while a test is executing. They
still directly test the strict comparison helper and every logical lifecycle
invariant. The standalone ordinary and multi-hour CI commands retain strict
process-count enforcement.

RSS is a bounded whole-process observation, not allocator attribution or proof
that every allocation was returned to the operating system. This deterministic
in-memory harness does not exercise provider sockets, external tasks, real-time
audio, Asterisk/provider interoperability, production load shape, rollback, or
live traffic. Those remain separate gates; Rust traffic stays disabled and
Asterisk remains the fallback.
