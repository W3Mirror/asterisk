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
peak logical call and transaction counts, and final resource counts. Results
are deterministic and are correctness evidence, not calls-per-second, CPU, or
memory benchmarks.

Every pull request and push to `aistack/main` runs the 512-call smoke. Scheduled
CI additionally runs a larger 16,384-call/256-call-batch matrix. Future media,
WebSocket, real concurrency, process memory, file-descriptor, CPU, latency, and
multi-hour soak harnesses must remain separate so this fast correctness gate
does not make unsupported performance claims.
