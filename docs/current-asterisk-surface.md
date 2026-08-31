# Current Asterisk Surface

**Inventory date:** 2026-08-31  
**Repository baseline:** `aistack/main` (`Asterisk 22.10.1` source plus
deployment tooling)  
**Inventory status:** repository/configuration complete; live production
verification pending

This inventory records the Asterisk surface that the Rust migration must
replace or deliberately retain as a fallback. The active container runtime is
configured from `docker/etc-asterisk/`. The larger `configs/basic-pbx/` tree is
an upstream sample PBX configuration and is not mounted by `compose.yml`; it
must not be treated as production scope until an operator confirms that it is
used.

## Evidence boundary

The repository contains configuration and operator documentation for a demo
stack and a Meta WhatsApp Business Calling trunk. It does not contain sanitized
production packet captures, provider credentials, a call-flow export, or an
executable Rust workspace on `aistack/main`. The local verification environment
also did not have `.env.aistack`, a host `asterisk` CLI, or permission to inspect
the host firewall. The facts below therefore distinguish configured behavior
from behavior that still requires a controlled live probe.

## Configured call flows

| Flow | Direction | Provider/peer | Signaling | Media | Current behavior | Status |
| --- | --- | --- | --- | --- | --- | --- |
| Demo endpoint `6001` → extension `100` | inbound from a registered client | local SIP/WebRTC client | SIP over UDP `5060` or WebSocket `transport-ws` on HTTP `8088` | RTP for native SIP; WebSocket media is available through `chan_websocket` | `Answer()` → `Playback(hello-world)` → `Hangup()` in `[from-internal]` | configured demo smoke path |
| Demo endpoint `6001` → AI media | inbound from a registered client | local SIP/WebRTC client | same as above | `chan_websocket` `/media/<connection_id>`, `media` subprotocol | extension `200` and `Dial(WebSocket/INCOMING/c(ulaw))` are commented templates | not active |
| Meta WhatsApp Business Calling → Asterisk | inbound | Meta, AS32934 | direct SIP-TLS `sip-trunk.w3.run:5061`, `transport-tls` | mandatory SRTP; DTLS-SRTP preferred, SDES alternative | `[from-meta]` answers, plays `hello-world`, and hangs up for `+`-prefixed destinations | configured, live DNS/certificate/call proof pending |
| Asterisk → Meta WhatsApp Business Calling | outbound | Meta, AS32934 | an explicit `sip:` URI over `transport-tls` is documented | SRTP policy is available on `[meta-wa]` | outbound dialplan is only a commented reference; no active origination route | not wired |

### Demo endpoint and dialplan

`docker/etc-asterisk/pjsip.conf` defines one endpoint/auth/AOR triplet named
`6001`. It allows only `ulaw` and `alaw`, keeps media anchored in Asterisk with
`direct_media=no`, permits five contacts, and evicts an old contact when the
limit is reached. The endpoint enters `[from-internal]`.

`docker/etc-asterisk/extensions.conf` currently contains only the active
extension `100`. The WebSocket bridge at extension `200` is intentionally a
template, not a production AI route. No active dialplan path originates a
provider call, transfers a call, records a call, or invokes an ARI/Stasis
application.

### Meta trunk

The trunk has no provider registration or SIP username/password. Its network
boundary is the `DOCKER-USER` `META-SIP` firewall chain populated by
`docker/scripts/meta-allowlist.sh` from AS32934 IPv4 routes. SIP endpoint
selection uses `match_header=From: /wa\.meta\.vc/`; this is routing, not
authentication, so the allowlist is security-critical.

The TLS transport uses `/etc/asterisk/secrets/tls/{fullchain,privkey}.pem`,
TLS 1.2, and fixed external signaling/media address `195.201.246.125`. mTLS is
disabled because Meta does not present a client certificate. The repository
documentation records that DNS was not live and the certificate was
self-signed on 2026-08-13; this must be rechecked before any live call claim.

The endpoint allows `ulaw`, `alaw`, `g722`, and `opus` (last). There is no
`codec_opus.so` in the documented build, so Opus is pass-through only and
cannot be transcoded. `media_encryption_optimistic=no` rejects a call that
cannot negotiate SRTP. `rtp_symmetric=yes`, `force_rport=yes`,
`rewrite_contact=yes`, and `direct_media=no` account for Docker bridge NAT.

## Protocol and media inventory

| Capability | Repository evidence | Migration implication |
| --- | --- | --- |
| SIP transports | UDP `0.0.0.0:5060`; WebSocket on HTTP `:8088`; public TLS `0.0.0.0:5061` for Meta | Rust must cover UDP, WS signaling, and TLS transport or retain Asterisk for the TLS path |
| SIP methods | `REGISTER` and `INVITE` are explicit in endpoint/trunk documentation; the repository has no production PCAP corpus | Capture and classify `ACK`, `BYE`, `CANCEL`, `OPTIONS`, provisional responses, `PRACK`/`UPDATE`, `REFER`, `NOTIFY`, and any provider-specific methods before cutover |
| Authentication | Digest user/password for `6001`; Meta network allowlist plus header-based routing; ARI Basic Auth; Caddy Basic Auth | Keep credentials out of Rust logs and preserve separate SIP/control-plane boundaries |
| Codecs | `6001`: `ulaw`, `alaw`; Meta: `ulaw`, `alaw`, `g722`, `opus` pass-through | Start with G.711; explicitly decide whether G.722 and Opus pass-through are required |
| DTMF | No DTMF option is set in the active Docker endpoint; the inactive `configs/basic-pbx` sample uses `rfc4733` | Live capture and provider confirmation required; test RFC 4733 and any SIP INFO fallback before migration |
| Early media | No active `Progress()`/`183` dialplan or provider capture is present | Treat early-media behavior as unknown and add a fixture/test before canary |
| Transfers/bridges | No active transfer or bridge application in the Docker dialplan; WebSocket bridge is commented | Confirm whether REFER, attended/blind transfer, and human-leg bridging are production requirements |
| Recording/CDR | No recording application or CDR backend is configured under `docker/etc-asterisk`; writable spool/log volumes exist | Confirm retention, format, storage owner, and post-call finalization requirements |
| NAT/RTP | Docker bridge NAT; Meta advertises fixed host address; native RTP range `10000–10100`; WebSocket media avoids that range | Validate advertised Contact/SDP and capacity with real topology; do not infer capacity from the port-range comment alone |
| RTP topology | WS signaling and media traverse Caddy/HTTP; Meta uses direct published SIP-TLS plus UDP SRTP; portal uses ARI | Rust cutover needs an explicit per-flow routing and rollback map |

## Control, observability, and external hooks

The runtime has these non-call dependencies:

- **ARI:** Asterisk HTTP `:8088/ari`, one server-side `aistack` user, and a
  read-only portal consuming `/asterisk/info`, `/channels`, `/endpoints`, and
  `/events`.
- **Caddy:** the only normal host entry point (`7231` on loopback/Tailscale),
  reverse-proxying portal, Grafana, ARI, SIP-over-WebSocket, media WebSockets,
  and `/metrics`. The public `sip.w3.run` host is restricted to ARI/WS/media.
- **Cloudflare Tunnel:** `cloudflared` provides the public `sip.w3.run` route
  from an outbound connection and depends on `TUNNEL_TOKEN` in the ignored
  `.env.aistack` file.
- **Metrics/logging:** Prometheus scrapes `asterisk:8088/metrics`; Grafana
  queries Prometheus and Loki; Promtail reads container logs through the
  read-only Docker socket. Asterisk and Caddy logs are JSON/console-backed,
  with no call-history database configured.
- **Certificates/firewall:** `issue-cert.sh` installs the SIP-TLS certificate
  and reloads PJSIP; `meta-allowlist.sh` refreshes the AS32934 allowlist weekly
  and at reboot. Both are host-operational hooks, not Asterisk APIs.
- **Deployment:** Docker Compose, named spool/log/observability volumes, a
  non-root Asterisk container, and bind-mounted read-only configuration.

No AMI, AGI, external queue, database, or application write API is wired into
the active Docker stack. The inactive `configs/basic-pbx` sample references
queues, voicemail, CDR custom output, and a DCS trunk; each requires explicit
production confirmation before it enters the Rust scope.

## Gaps required to close Phase 0

1. Confirm which deployment (Docker stack, native service, or another host) is
   production and export its effective configuration.
2. Verify `sip-trunk.w3.run` DNS, the CA-valid certificate, firewall chain, and
   Meta onboarding state from an external vantage point.
3. Obtain sanitized inbound and outbound captures for every provider/call
   flow, including failed calls, retransmissions, provisional responses,
   DTMF, transfers, and early media.
4. Confirm recording/CDR ownership, retention, AI media endpoint behavior, and
   post-call event consumers.
5. Determine whether the sample `configs/basic-pbx` DCS/queue/voicemail flows
   are deployed anywhere; exclude them only after an operator sign-off.
6. Measure a baseline for call setup latency, packet loss/jitter, concurrent
   calls, memory, and cleanup time. The `10000–10100` RTP range is a
   configuration estimate, not a measured capacity limit.

Until these gaps are closed, the Rust migration scope is limited to the
configured demo/Meta primitives and the control/observability contracts listed
above, with Asterisk remaining the fallback.
