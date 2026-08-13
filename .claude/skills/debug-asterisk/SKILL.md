---
name: debug-asterisk
description: Debug the AIStack Asterisk/SIP stack — where logs live, how to trace SIP/WebSocket/call failures, and which CLI/API to use for each symptom
---

# Debugging the AIStack Asterisk/SIP stack

## Stack map

8 services on the `aistack` docker network (see `compose.yml`): `asterisk`,
`prometheus`, `grafana`, `loki`, `promtail`, `portal`, `caddy`,
`cloudflared`. `caddy` publishes `7231:80`, bound explicitly to
`127.0.0.1` and the tailscale IP (`100.69.165.55:7231`), *not* `0.0.0.0`.
`asterisk` ALSO publishes two ports, deliberately to `0.0.0.0` (unlike
caddy) — `5061:5061/tcp` and `10000-10100:10000-10100/udp` — for the
public Meta WhatsApp SIP-TLS trunk (see below); access control there is
an iptables allowlist, not interface binding. Everything else (including
`cloudflared`, which makes an outbound-only connection to Cloudflare's
edge) is internal-only; reach it by exec'ing into a container or curling
from one already on the network.

Public exposure for `sip.w3.run` goes through `cloudflared`, not the
published port — its tunnel ingress (Cloudflare Zero Trust dashboard)
points at `http://caddy:80` over the `aistack` network. `docker/caddy/Caddyfile`
has a dedicated `http://sip.w3.run` site block that only proxies `/ws`,
`/media/*`, `/ari*` (no basic auth); everything else 404s under that host.
All other `Host` headers (localhost, tailscale IP) keep the full catch-all
below. `cloudflared` reads its token from `TUNNEL_TOKEN` in `.env.aistack`
(gitignored) — it crash-loops harmlessly until a real token is set.

Auth layers:
- **Basic auth** (`admin` / password stored outside repo, hash in
  `CADDY_ADMIN_HASH` env var) gates every route through Caddy's catch-all
  site block **except** `/ari*`, `/ws`, `/media/*` (see
  `docker/caddy/Caddyfile`) — those are left open because SIP/WebRTC
  clients and ARI can't attach an `Authorization` header on their own auth
  layer. None of it applies under `Host: sip.w3.run` — that block only
  ever exposes the open `/ari*`/`/ws`/`/media/*` routes and 404s everything
  else.
- **ARI** has its own basic auth inside Asterisk: user `aistack` (see
  `docker/etc-asterisk/ari.conf`, which `#include`s
  `secrets/ari_secret.conf` — gitignored, not committed). Password kept in
  sync with `.env.aistack`'s `ARI_PASSWORD` (read by the portal at
  runtime, `env_file:` on compose.yml's portal service).
- **SIP endpoint 6001** — the one configured extension in
  `docker/etc-asterisk/pjsip.conf` (`auth=6001`; the `type=auth` block
  `#include`s `secrets/pjsip_auth.conf`, gitignored). Dialplan context
  `from-internal`; only extension 100 does anything live (plays
  hello-world for an audio/RTP smoke test).

## Log locations

| Source | How to see it |
|---|---|
| Any service, live tail | `docker compose logs -f <service>` (asterisk, caddy, grafana, loki, promtail, portal, prometheus) |
| Asterisk full log (file, includes debug) | Inside the container: `/var/log/asterisk/full` (named volume `asterisk-log`, mounted per `docker/etc-asterisk/logger.conf`'s `full =>` line) |
| Asterisk console output | `docker compose logs asterisk` — same content as CLI console: notice/warning/error/verbose (see `logfiles` in `logger.conf`) |
| Caddy access log | JSON on stdout, `docker compose logs caddy` — one line per request, includes status code, path, `user_id`, and 401s |
| Centralized / cross-service | Grafana → Explore, datasource **Loki** (uid `loki`), or the provisioned **Logs** dashboard (`docker/grafana/provisioning/dashboards/logs.json`) |

Example LogQL in Grafana Explore:
```
{container="caddy"}                     # all caddy access log lines
{container="caddy"} |= "\"status\":401"  # just the 401s
{container="asterisk"} |= "WARNING"
{container="asterisk"} |= "NOTICE"
```
(`container` label comes from promtail's docker_sd_configs scrape, see
`docker/promtail/promtail.yml` — it's filtered to this compose project's
containers, so unrelated containers on the host don't leak in.)

## Symptom → tool mapping

**SIP registration / call failures**
```
docker compose exec asterisk asterisk -rx "pjsip set logger on"
docker compose logs -f asterisk          # watch the SIP dialog
docker compose exec asterisk asterisk -rx "pjsip set logger off"   # turn it back off — verbose
```

**Endpoint / registration state**
- Portal UI, or:
```
docker compose exec asterisk asterisk -rx "pjsip show endpoints"
docker compose exec asterisk asterisk -rx "pjsip show aors"
docker compose exec asterisk asterisk -rx "pjsip show contacts"
```

**WebSocket / media issues** (`/ws`, `/media/*`)
- Check Caddy's access log for the upgrade request: a `101` means the
  websocket handshake succeeded; a `4xx` means it never got there
  (auth/routing problem upstream of Asterisk).
- Then check Asterisk's own view:
```
docker compose exec asterisk asterisk -rx "http show status"
docker compose exec asterisk asterisk -rx "module show like websocket"
```

**Dialplan**
```
docker compose exec asterisk asterisk -rx "dialplan show from-internal"
docker compose exec asterisk asterisk -rx "dialplan reload"
```

**ARI issues**
```
curl -u aistack:<ari-password> http://localhost:7231/ari/asterisk/info
```
(`/ari*` is NOT behind Caddy's basic auth — only ARI's own.) Also check
the portal's `/api/events` SSE endpoint, which proxies ARI's event
websocket to the browser.

**Metrics gaps**
```
curl -u admin:<caddy-password> http://localhost:7231/metrics
```
Then check Prometheus's own view of the scrape target:
`docker compose exec prometheus wget -qO- http://localhost:9090/api/v1/targets`
(`docker/etc-asterisk/prometheus.conf` exposes `/metrics` on 8088, with
auth commented out there — Caddy is what actually gates it externally.)

**Meta WhatsApp SIP-TLS trunk (`sip-trunk.w3.run:5061`)**
- Full writeup: `docs-internal/reference/meta-trunk.mdx`.
- This is a SEPARATE public entry point from `sip.w3.run` above -- different
  hostname, port (5061 not 80/443), transport (raw SIP-TLS not WebSocket),
  and access control (iptables allowlist, not Caddy basic auth/tunnel).
  Don't confuse the two when debugging.
```
docker compose exec asterisk asterisk -rx "pjsip show transports"   # transport-tls bound on 0.0.0.0:5061?
docker compose exec asterisk asterisk -rx "pjsip show endpoints"    # meta-wa present, Identify: meta-wa-identify/meta-wa
docker compose exec asterisk asterisk -rx "pjsip show identifies"   # match_header=From: /wa\.meta\.vc/
docker compose exec asterisk asterisk -rx "pjsip set logger on"     # trace the actual INVITE -- confirms whether match_header matched
```
- **Call never arrives at all**: check the firewall allowlist first, before
  touching Asterisk config -- a genuinely-Meta call from an IP not yet in
  the allowlist (AS32934 ranges rotate) looks identical to "nothing
  happened" from Asterisk's side, because it's dropped before reaching the
  container:
  ```
  iptables -L META-SIP -n -v          # counters on the DROP rules moving = something is being blocked
  ipset list meta-wa-v4 | grep <ip>   # is this specific source IP currently allowlisted?
  sudo docker/scripts/meta-allowlist.sh   # force a refresh (safe, idempotent, also runs weekly + @reboot via cron)
  ```
- **Call arrives but gets no endpoint / ends up on `s`/default context**:
  the `From` header didn't match `wa.meta.vc` via the `[meta-wa-identify]`
  regex -- `pjsip set logger on` and inspect the actual `From:` line Meta
  sent.
- **TLS handshake fails**: check which cert is currently installed --
  `openssl x509 -in docker/etc-asterisk/secrets/tls/fullchain.pem -noout -subject -issuer -dates`.
  A self-signed placeholder (`issuer == subject`) means
  `docker/scripts/issue-cert.sh` either hasn't been run yet or failed
  (commonly: DNS for `sip-trunk.w3.run` not resolving to this host's
  public IP yet -- certbot's HTTP-01 challenge can't complete).
- **Media negotiation fails (SRTP/DTLS)**: `pjsip set logger on` and check
  the SDP for `a=crypto`/`a=fingerprint` lines; `media_encryption_optimistic=no`
  on `[meta-wa]` means a failed SRTP negotiation rejects the call outright
  rather than falling back to plain RTP -- that's intentional, not a bug.
- **Codec negotiation fails on an opus-only leg**: this build has
  `res_format_attr_opus.so` (passthrough negotiation) but no
  `codec_opus.so` (no transcoding) -- confirmed via
  `docker compose exec asterisk asterisk -rx "module show like opus"`.
  A call that needs opus transcoded to/from something else will fail;
  this is a known gap, not something to "fix" via config.

**Container-level**
```
docker compose ps                # all should be Up; only caddy has a host port
docker compose logs <service>
docker compose restart <service>
```

## Reload cheat-sheet

- **PJSIP config changes** (`pjsip.conf`): there is **no standalone
  `pjsip reload`** CLI command in this build — only narrower ones like
  `pjsip reload qualify endpoint`. Use:
  ```
  docker compose exec asterisk asterisk -rx "module reload res_pjsip.so"
  docker compose exec asterisk asterisk -rx "pjsip show endpoints"   # verify
  ```
- **Dialplan changes**: `dialplan reload`
- **Full restart** (last resort, drops calls): `core restart now`
- **Compose lifecycle**: `docker compose up -d` picks up compose.yml
  changes; bind-mounted config files (`docker/etc-asterisk/*`,
  `docker/caddy/Caddyfile`, `docker/promtail/promtail.yml`,
  `docker/loki/loki-config.yml`) are read fresh on container start, so a
  plain `restart` of that one service is usually enough — **except
  Caddy**, see below.

## Known gotchas

- **`NEXT_PUBLIC_*` is build-time only.** Next.js inlines these at `next
  build`; changing them under `environment:` in compose.yml does nothing
  to an already-built portal image — they must also be passed as a build
  `arg` and the image rebuilt.
- **`handle` vs `handle_path` for `/grafana`.** Caddy's `handle_path`
  strips the prefix before proxying; Grafana is configured with
  `GF_SERVER_SERVE_FROM_SUB_PATH=true` and expects to see `/grafana` in
  the request path, so stripping it causes an infinite redirect loop. Use
  `handle` (no stripping) for that route.
- **Caddy reload sees a stale inode.** After editing the bind-mounted
  `Caddyfile`, `docker compose exec caddy caddy reload` can silently keep
  the old config — do `docker compose restart caddy` instead.
- **ufw does not filter Docker's DNAT'd traffic.** Docker inserts its own
  iptables DNAT/FORWARD rules for published ports ahead of ufw's chain, so
  a ufw rule alone will not block traffic to a published container port —
  don't rely on it as the only guard for `7231`.
- **Port 3000 may already be in use on the host.** This is why `portal`
  and `grafana` are not published directly — both listen on 3000
  internally and only Caddy fronts them externally, avoiding any host
  port collision.
