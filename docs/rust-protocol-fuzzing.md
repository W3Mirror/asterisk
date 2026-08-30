# Rust protocol fuzzing

The `fuzz/` package contains `cargo-fuzz` targets for every safe wire parser
used by the Rust call and media layers:

| Target | Parser |
| --- | --- |
| `sip_parse` | `sip-parser::parse` |
| `sdp_parse` | `sdp::parse` |
| `rtp_parse` | `rtp::parse` |
| `rtcp_parse` | `rtcp::parse` |
| `dtmf_parse` | `dtmf::parse` |
| `websocket_parse` | `media_websocket::WebSocketFrame::decode` |

Each target passes arbitrary bytes directly to a bounded parser and discards
the `Result`. Parser errors are expected; a panic, sanitizer finding, or
resource exhaustion is a failure.

## Local checks

Install `cargo-fuzz` once, then from the repository root run:

```bash
cargo +nightly fuzz list --fuzz-dir fuzz
cargo +nightly fuzz check --fuzz-dir fuzz
```

The default command enables the address sanitizer and therefore requires a
nightly toolchain. On a stable toolchain where the sanitizer or `cfg(fuzzing)`
setup is unavailable, the harnesses can still be type-checked with:

```bash
cargo fuzz check --fuzz-dir fuzz --sanitizer none --no-cfg-fuzzing
```

Run a bounded smoke pass for one target with:

```bash
cargo +nightly fuzz run --fuzz-dir fuzz sip_parse --sanitizer address --no-cfg-fuzzing -- \
  -runs=100 -max_len=65535 -timeout=5
```

Replace `sip_parse` with any target from `cargo +nightly fuzz list`. Keep generated
`fuzz/target/`, crash artifacts, and local corpora out of commits unless a
minimized regression input is intentionally added to a target-specific corpus.

The harnesses are offline and provider-neutral. They do not open sockets,
require credentials, or enable the Rust routing path.
