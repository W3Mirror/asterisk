# Rust SIPp integration testing

The local SIPp harness drives the real blocking UDP `CallRuntime` boundary from
an isolated SIPp 3.7.2 container. It is provider-neutral and does not require
Asterisk credentials, production routing, or external SIP connectivity.

The ordinary pull-request and `aistack/main` workflow runs three deterministic
inbound scenarios:

- ringing, answer, ACK, and caller BYE;
- a `486 Busy Here` final failure and ACK;
- caller CANCEL with `200`/`487` completion and ACK.

Each scenario also verifies that the runtime reaches a terminal call state and
reclaims the call before the fixture exits. Run the same matrix locally with:

```text
tests/rust-sipp/run.sh
```

The runner requires Docker on Linux because the SIPp container uses host
networking to reach the Rust UDP fixture. The image is built from a
digest-pinned Ubuntu 24.04 base with the SIPp package pinned to 3.7.2. No
provider or Asterisk endpoint is contacted. Set `SIPP_PORT` when the default
fixture port `15060` is unavailable; SIPp uses the following port locally.
