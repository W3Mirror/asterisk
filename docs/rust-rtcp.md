# Rust RTCP session

`rtcp::RtcpSession` wraps the existing bounded, provider-neutral RTCP parser
for socket-backed receive paths. It records sent and accepted packet/octet
counts, arrival time, and known SSRC changes, and can optionally require an
expected remote SSRC. `send` applies the same configured datagram bound before
advancing send metrics.

Use `new_with_source_policy` or `with_source_policy` to apply the shared
`sip-security::SourceIpPolicy`. `receive_from` evaluates the observed
`SocketAddr` before size checks or parsing. A denied peer therefore cannot
increment invalid counters or change SSRC/receive state. The legacy `parse`
function remains available for callers that perform source authorization at a
lower transport boundary.

This slice is offline and provider-neutral. It does not enable Rust media
traffic, alter Asterisk configuration, or claim RTCP interoperability with a
live carrier.
