---
name: debug-asterisk
description: Debug the AIStack Asterisk/SIP stack — where logs live, how to trace SIP/WebSocket/call failures, and which CLI/API to use for each symptom
---

# Debugging the AIStack Asterisk/SIP stack

## Stack map

7 services on the `aistack` docker network (see `compose.yml`): `asterisk`,
`prometheus`, `grafana`, `loki`, `promtail`, `portal`, `caddy`. **Only
`caddy` publishes a host port** — `7231:80`, reachable at the tailscale IP
`100.69.165.55:7231` (or `localhost:7231` from the host). Everything else
is internal-only; reach it by exec'ing into a container or curling from
one already on the network.

Auth layers:
- **Basic auth** (`admin` / password stored outside repo, hash in
  `CADDY_ADMIN_HASH` env var) gates every route through Caddy **except**
  `/ari*`, `/ws`, `/media/*` (see `docker/caddy/Caddyfile`) — those are
  left open because SIP/WebRTC clients and ARI can't attach an
  `Authorization` header on their own auth layer.
- **ARI** has its own basic auth inside Asterisk: user `aistack` (see
  `docker/etc-asterisk/ari.conf`), password in compose.yml's portal env.
- **SIP endpoint 6001** — the one configured extension in
  `docker/etc-asterisk/pjsip.conf` (`auth=6001`, password in that file).
  Dialplan context `from-internal`; only extension 100 does anything live
  (plays hello-world for an audio/RTP smoke test).

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
