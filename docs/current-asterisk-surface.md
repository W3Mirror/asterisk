# Current Asterisk surface

Status: Phase 0 inventory, recorded 2026-08-30 (UTC)
Repository: `W3Mirror/asterisk`, branch `aistack/main`, source baseline
`Asterisk 22.10.1`
Inventory commit: `251c42618c4c5a07ccc84550cb09a82b63662901`

This is the evidence boundary for the Rust SIP/RTP migration. It describes what
the repository configures and what was observable from the current host. A
checked-in config is not proof that a production call path is enabled, and a
source module is not proof that it was built or loaded. Items without runtime,
provider, or sanitized packet evidence remain **unverified**.

## Evidence and status vocabulary

- **configured** — an uncommented repository configuration or route defines the
  behavior.
- **reference-only** — present only in comments, examples, or documentation; it
  is not an active call path in this checkout.
- **not configured** — the relevant configuration or integration was not found.
- **unverified** — production/runtime state, provider behavior, or negotiated
  protocol details cannot be established from this checkout.

Repository evidence was collected from `compose.yml`,
`docker/etc-asterisk/*.conf`, `docker/caddy/Caddyfile`, `portal/`, the Dockerfile,
and the internal operations/reference pages. The live checks were read-only:

```text
command -v asterisk; asterisk -V
docker compose ps
ss -ltnup
dig +short sip-trunk.w3.run @1.1.1.1
hostname -f; ip -brief address
```

The current host returned no `asterisk` binary, no running Compose stack, and no
listener on the SIP, RTP, or Asterisk HTTP ports. `docker compose ps` could not
load the stack because `.env.aistack` is absent. At capture time the host
interfaces were `135.181.5.36/32` and Tailscale `100.99.75.85/32`; DNS returned
`65.1.135.111` for `sip-trunk.w3.run`.

## Runtime topology in the repository

The Asterisk service is built from this source tree as `asterisk:22.10.1-aistack`
on an Ubuntu 24.04 runtime and runs as the unprivileged `asterisk` user. Its
configuration directory is mounted read-only from `docker/etc-asterisk/`.

| Surface | Repository configuration | Current status / implication |
| --- | --- | --- |
| SIP/UDP | PJSIP `transport-udp`, `0.0.0.0:5060` | configured inside the container; Compose publish is commented out, so external reachability is unverified |
| SIP over WebSocket | PJSIP `transport-ws` on the Asterisk HTTP server (`:8088`), Caddy `/ws`, subprotocol `sip` | configured; Caddy is the edge, but the tunnel token/runtime is unverified |
| SIP/TLS | PJSIP `transport-tls`, `0.0.0.0:5061`, Compose `5061:5061/tcp` | configured for the Meta experiment; DNS and CA-valid certificate are not live in the current probe |
| Native RTP/SRTP | `rtpstart=10000`, `rtpend=10100`, Compose `10000-10100/udp` | configured/published for the Meta experiment; no active listener was observed |
| Asterisk HTTP/ARI/metrics | `http.conf` binds `0.0.0.0:8088`; `/ari`, `/metrics`, and WebSocket routes are proxied by Caddy | configured; no running process was available to validate it |
| Control portal | Next.js BFF at `portal:3000`, Caddy catch-all behind basic auth | read-only health/channels/endpoints UI; no call-control mutation route is present |
| Cloudflare edge | `cloudflared` connects outbound to `caddy:80`; hostname routing is dashboard state | repository wiring exists; `.env.aistack` and tunnel state are unverified |

The repository also contains a prior Kamailio/rtpengine VPS plan, but no
Kamailio, rtpengine, WireGuard, or SBC configuration is present in this source
tree. That plan is not an active call path.

## Call-flow inventory

These are the concrete flows defined by the current checkout. “Production use”
is intentionally separate from “configured here.”

| ID | Direction and peer | Signaling/media path | Call handling | Status |
| --- | --- | --- | --- | --- |
| F-001 | Registered SIP client `6001` to Asterisk | UDP `:5060` or SIP-over-WS `/ws`; SIP digest credentials are included from the ignored secret file | `from-internal`, extension `100`: `Answer()` → `Playback(hello-world)` → `Hangup()` | configured demo smoke test; production use unverified |
| F-002 | Registered SIP client to an AI/media WebSocket | SIP leg as F-001; media would use `chan_websocket` `/media/<connection_id>` | Extension `200` and `Dial(WebSocket/INCOMING/c(ulaw))` are commented out | reference-only; no active AI media bridge |
| F-003 | Meta WhatsApp Business Calling inbound to the configured number | SIP-TLS `sip-trunk.w3.run:5061`; endpoint selected by `From: /wa\.meta\.vc/`; DTLS-SRTP required | `from-meta`, pattern `_+X.`: `Answer()` → `Playback(hello-world)` → `Hangup()` | configured placeholder/demo; DNS, certificate, runtime, and real call evidence are unverified |
| F-004 | Asterisk-originated call toward Meta | Intended `PJSIP/sip:+<number>@wa.meta.vc\;transport=tls@meta-wa` dial string | Only a commented example exists; no outbound AOR, registration, route, or API handler is configured | reference-only; not wired |
| F-005 | Meta caller to AI agent | Intended replacement of F-003's playback with `Dial(WebSocket/INCOMING/c(ulaw))` or `Stasis(...)` | The AI handoff is comments only; no application owns the call | reference-only; not wired |
| F-006 | Operator browser to Asterisk control plane | Portal server calls ARI over `http://asterisk:8088/ari`; ARI events become browser SSE | `GET /api/health`, `/api/channels`, `/api/endpoints`, `/api/events`; no answer, hangup, transfer, DTMF, or recording API route | configured read-only observability path |
| F-007 | Browser/softphone media edge | Caddy routes `/ws`, `/media/*`, and `/ari*`; SIP/media paths are exempt from Caddy basic auth and rely on their own protocol/token boundaries | No browser call fixture or authenticated end-to-end test is checked in | configured edge routes; runtime behavior unverified |

No other active inbound context, outbound carrier route, or application-owned
call flow was found in the checked-in configuration. The full Asterisk source
contains many PBX/channel modules, but source presence alone is not an
inventory of production use.

## Provider and endpoint inventory

### Meta WhatsApp Business Calling (repository-described)

The only external provider named in the checkout is Meta WhatsApp Business
Calling:

- signaling hostname/port: `sip-trunk.w3.run:5061` over TLS 1.2;
- endpoint: `[meta-wa]`, selected by the `From` header domain `wa.meta.vc`;
- advertised signaling/media address in the checked-in PJSIP config:
  `195.201.246.125`;
- media: `media_encryption=dtls`, `media_encryption_optimistic=no`,
  `dtls_setup=actpass`, no mTLS;
- codecs listed: PCMU (`ulaw`), PCMA (`alaw`), G.722, and Opus last;
- NAT-related endpoint options: `rtp_symmetric=yes`, `force_rport=yes`,
  `rewrite_contact=yes`, `direct_media=no`;
- firewall helper: `docker/scripts/meta-allowlist.sh`, intended to restrict
  the published SIP/RTP ports to AS32934 IPv4 ranges.

This is a repository declaration, not proof of a live provider relationship.
The source documentation says the certificate was a self-signed placeholder
until DNS was corrected; the current read-only probe still sees the wildcard
address `65.1.135.111`, not the configured `195.201.246.125`. The host's current
interface address is different again (`135.181.5.36`). The advertised address,
DNS, certificate, firewall state, and provider dashboard must be reconciled
before this path can be considered a production dependency.

### Local SIP endpoint

Endpoint `6001` is the only non-provider PJSIP endpoint configured in the
checkout. It uses username/password digest authentication from the ignored
`docker/etc-asterisk/secrets/pjsip_auth.conf`, accepts `ulaw` and `alaw`, stores
up to five contacts, and enters `from-internal`. It is a demo/local endpoint,
not evidence of a carrier trunk.

### Other providers and external hooks

No additional provider hostname, outbound registration, AOR, carrier
credential, webhook, AMI, AGI, or external call-control integration was found in
the checked-in runtime configuration. Whether production uses any of these is
**unverified** and is a Phase 0 data-collection item, not a conclusion that
they are unused.

## Signaling, media, and call-control surface

### SIP and transport

The active configuration names UDP, WebSocket, and TLS transports as above.
There is no checked-in provider scenario or packet fixture proving which SIP
methods are actually exchanged. The local and Meta configurations imply
`REGISTER`/`INVITE` on their respective ingress paths and normal dialog
`ACK`/`BYE` handling; `CANCEL`, `OPTIONS`, `REFER`, `NOTIFY`, `INFO`, `UPDATE`,
and `PRACK` are not demonstrated by a fixture or an uncommented dialplan route.
Treat the source module set as implementation capability, not observed
provider behavior, until sanitized captures are collected.

### SDP, codecs, and RTP

- `6001` offers PCMU/PCMA only.
- `meta-wa` lists PCMU, PCMA, G.722, and Opus. The checked-in build notes say
  Opus attribute support is present but a transcoding `codec_opus.so` is not;
  actual image contents still require a fresh build/runtime check.
- Meta is configured for mandatory DTLS-SRTP. The rest of the stack has no
  explicit media-encryption setting.
- Native RTP uses the bounded 101-port range `10000–10100`; the WebSocket media
  path does not use this range.
- No RTP/RTCP packet capture, negotiated SDP sample, packet-loss/jitter sample,
  or media soak result is checked in.

### DTMF

The source tree includes PJSIP DTMF support (`res_pjsip_dtmf_info.c`) and the
generic `SendDTMF` application, but no endpoint `dtmf_mode`, dialplan
`SendDTMF`, provider fixture, or captured telephone-event exchange is present in
the runtime configuration. Receive, emit, deduplication, and provider-specific
SIP INFO behavior are therefore unverified.

### Recording, bridging, and transfers

The source tree contains recording and bridge-related modules, but the checked-
in dialplan has no `MixMonitor()`, `Monitor()`, bridge, transfer, or `REFER`
route. The only bridge-shaped examples are the commented WebSocket `Dial()`
lines. Recording channels, storage, transfer semantics, and human escalation
are unverified production requirements.

### NAT and address handling

Only the Meta endpoint has symmetric RTP/contact rewriting enabled. Its
`external_signaling_address` and `external_media_address` are hardcoded to
`195.201.246.125`; they do not match the current host probe and must not be
treated as a current production value. No STUN/TURN/ICE or public RTP relay is
configured. The prior Kamailio/rtpengine plan is not implemented here.

### Authentication and security boundaries

- Local SIP credentials are digest-authenticated and kept in ignored files.
- Meta is intended to be constrained by the host `DOCKER-USER`/ipset
  allowlist plus the `From`-domain routing rule; the header rule is routing, not
  authentication.
- ARI has its own `aistack` basic-auth account, also sourced from an ignored
  secret and mirrored into `.env.aistack` for the portal.
- Caddy basic-auth protects portal, docs, Grafana, and metrics routes; `/ws`,
  `/media/*`, and `/ari/*` intentionally bypass that layer.
- TLS for Asterisk HTTP/WebSocket is commented out; the only configured TLS
  listener is the Meta experiment's PJSIP transport.

The absence of `.env.aistack` in the current checkout means no credential value
was inspected and no authenticated runtime path was tested.

## Fresh live-state recheck

A read-only recheck from the active PR18 worktree on 2026-08-30 found the same
deployment boundary, with the following concrete results:

- `command -v asterisk`, `asterisk -V`, and `pgrep -a asterisk` found no
  Asterisk binary or process;
- `docker compose ps` could not load the stack because the worktree has no
  `.env.aistack`;
- no listener was present on SIP UDP/TCP `5060`/`5061`, Asterisk HTTP/ARI
  `8088`, or the configured RTP range `10000–10100`; requests to
  `127.0.0.1:8088/` and `/ari/api-docs` were refused;
- `sip-trunk.w3.run` still resolves to `65.1.135.111`; TCP probes to ports
  `5060` and `5061` timed out, and an `openssl s_client` TLS probe to `5061`
  also timed out;
- the host reports `135.181.5.36/32` on `enp35s0` and `100.99.75.85/32` on
  Tailscale, so the repository's advertised `195.201.246.125` remains
  unreconciled;
- no sanitized SIP/RTP/RTCP/WebSocket capture or replay corpus is checked in.
  The only matching filename is the unrelated example
  `contrib/scripts/sipp-sendfax.xml`.

No credential contents, provider dashboard, production configuration, or live
traffic were inspected or modified. Provider interoperability and a valid
memory/load baseline therefore remain unavailable.

### Control plane, events, and identifiers

The portal is a read-only ARI BFF. It forwards ARI channel/endpoint/health data
and fans out ARI events over SSE; it does not expose the programmable call API
described by the Rust goal. No independent `call_*`, `leg_*`, `stream_*`, or
`event_*` application identifiers are defined in this repository. A Rust core
must therefore introduce those identifiers rather than adopting SIP Call-ID as
the application key.

### Observability and limits

`res_prometheus` exposes the Asterisk metrics endpoint, Prometheus scrapes it
every 10 seconds, and Loki/Promtail/Grafana collect container logs. The checked-
in dashboard is a reachability/uptime dashboard, not a call-quality or
per-call-diagnostic surface. The only explicit media resource bound is the
101-port RTP range; no configured call limit, transaction limit, WebSocket
queue limit, event queue limit, or load/soak result is present.

## Capability boundary for the Rust migration

The evidence supports a deliberately small first slice:

1. SIP UDP/TCP/TLS transport separation from call state;
2. safe SIP/SDP/RTP/RTCP parsing with size/header limits and fuzz harnesses;
3. G.711 PCMU/PCMA negotiation and RTP session accounting;
4. explicit DTMF behavior once provider captures establish the required mode;
5. a bounded bidirectional media interface replacing the commented
   `chan_websocket` handoff;
6. explicit call/dialog identifiers and lifecycle events; and
7. a configuration-level Rust/Asterisk routing switch so every rollout can
   return to Asterisk.

This is not yet a production scope baseline. Milestone 1 cannot exit until the
unknown provider/call-flow rows below are answered with runtime evidence.

## Required Phase 0 collection

The next collection run should be performed on the actual Asterisk host with
secrets redacted and no credential values copied into this repository:

```bash
asterisk -rx "pjsip show transports"
asterisk -rx "pjsip show endpoints"
asterisk -rx "pjsip show registrations"
asterisk -rx "pjsip show aors"
asterisk -rx "dialplan show"
asterisk -rx "core show channels"
asterisk -rx "module show"
asterisk -rx "http show status"
asterisk -rx "rtp show settings"
```

For every provider and flow, retain a sanitized successful and failed capture
with SIP headers, SDP, RTP/RTCP, DTMF, transfer, recording, NAT, and timeout
variants. Record the provider, direction, transport, methods/status codes,
codec/payload mapping, media topology, and external hooks. Establish idle,
per-call, setup-burst, disconnect/reconnect, and post-call memory baselines;
there is no valid baseline from the current host because Asterisk is not
running here.

### Phase 0 exit matrix

| Requirement | Evidence now | Status |
| --- | --- | --- |
| Current configured Asterisk surface | Repository inventory in this document | complete for checkout |
| Production provider inventory | No runtime/credential/provider dashboard access in current probe | incomplete |
| Target call-flow definitions | F-001–F-007, with configured vs reference-only labels | partial; production confirmation required |
| SIP/SDP/RTP/DTMF capture corpus | No `pcap`, SIPp, or replay fixture is checked in | not started |
| Performance/memory baseline | No Asterisk process or load result available | not started |

Until the incomplete rows have direct evidence, the Rust implementation should
remain in planning and fixture-collection mode, with Asterisk retained as the
compatibility fallback.
