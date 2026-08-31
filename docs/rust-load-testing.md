# Rust deterministic load and reclamation smoke

The `load-smoke` workspace package exercises bounded signaling capacity without
provider credentials, sockets, sleeps, or wall-clock assertions. It creates
unique inbound INVITEs in fixed-size batches, cancels every call, verifies the
expected `200` and `487` responses, reclaims every terminal call, and requires
both the call registry and SIP transaction count to return to zero before the
next batch reuses that capacity.

Run the ordinary PR-sized smoke locally with:

```sh
cargo run -p load-smoke --locked -- 512 32
```

The two arguments are total calls and maximum calls retained in one batch. A
successful report includes attempted/completed/failed counts, batch count,
peak logical call and transaction counts, final resource counts, elapsed call
throughput, and best-effort Linux resident-memory/file-descriptor observations.
The resident-growth-per-peak-call value is a coarse process-level observation,
not allocator attribution or a production memory SLO. Deterministic assertions
use logical resource counts; elapsed and process values remain environment
dependent.

Every pull request and push to `aistack/main` runs the 512-call smoke. Scheduled
CI additionally runs a larger 16,384-call/256-call-batch reclamation tier and a
dedicated exact signaling-capacity matrix:

```sh
tests/rust-signaling-capacity/run.sh
```

The script defaults to the required 1,000/5,000/10,000 tiers. Explicit smaller
tiers may be supplied for a quick local probe, for example
`tests/rust-signaling-capacity/run.sh 8 16`.

Each matrix process creates its complete concurrency level in one bounded batch,
then cancels and reclaims every call before exit. The harness checks the full
batch's registry size once instead of cloning the growing registry after every
INVITE, so the measurement is not dominated by observer-induced quadratic
work.

This matrix remains single-process, single-threaded, in-memory signaling-only
evidence. The separate [combined signaling/media smoke](rust-combined-load-smoke.md)
holds synthetic calls and real bounded media sessions at the same time, but
neither harness establishes real simultaneous network traffic, calls per second
at a provider boundary, setup/teardown latency percentiles, CPU per call,
same-level media or WebSocket capacity, or a production SLO. The separate
[mixed-lifecycle soak](rust-lifecycle-soak.md) adds repeated lifecycle and
bounded process-resource evidence. Provider/Asterisk evidence remains required
before Rust traffic can be enabled.
