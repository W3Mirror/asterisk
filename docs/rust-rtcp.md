# Rust RTCP session

`rtcp::RtcpSession` wraps the existing bounded, provider-neutral RTCP parser
for socket-backed receive paths. It records sent and accepted packet/octet
counts, the latest reported cumulative loss and jitter, arrival time, and
known SSRC changes, and can optionally require an expected remote SSRC. `send`
applies the same configured datagram bound before advancing send metrics.

When a Sender Report is received, the session retains its NTP middle-32-bit
identifier and local arrival time. A later reception report with that matching
LSR produces an RTT estimate by subtracting DLSR from the elapsed local time.
The estimate is exposed only when a matching report exists; it does not claim
wall-clock synchronization or provider interoperability.

`reception_report_timing` exposes the latest accepted Sender Report identifier
and its monotonic delay in RTCP 16.16-second units. `MediaSession` combines that
timing with the current RTP source SSRC, extended sequence, observed loss, and
jitter to generate a Receiver Report. Report fractions are interval-scoped and
reset when the RTP source SSRC changes; cumulative loss and sequence/jitter
values remain scoped to the current RTP source.

Use `new_with_source_policy` or `with_source_policy` to apply the shared
`sip-security::SourceIpPolicy`. `receive_from` evaluates the observed
`SocketAddr` before size checks or parsing. A denied peer therefore cannot
increment invalid counters or change SSRC/receive state. The legacy `parse`
function remains available for callers that perform source authorization at a
lower transport boundary.

This slice is offline and provider-neutral. It does not enable Rust media
traffic, alter Asterisk configuration, or claim RTCP interoperability with a
live carrier.
