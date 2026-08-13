# Asterisk Portal

A read-only Next.js management dashboard for the Asterisk 22 server in this
repo: live active channels, endpoint status, and system health, backed by a
server-side BFF that talks to Asterisk's ARI over REST + WebSocket so
credentials never reach the browser.

## Environment variables

| Variable | Default | Notes |
| --- | --- | --- |
| `ARI_BASE_URL` | `http://asterisk:8088/ari` | ARI REST base URL. The events WebSocket URL is derived from this (http→ws, https→wss). |
| `ARI_USERNAME` | `aistack` | ARI basic-auth username. |
| `ARI_PASSWORD` | `changeme-ari` | ARI basic-auth password. Server-side only — never sent to the client. |
| `NEXT_PUBLIC_GRAFANA_URL` | `http://localhost:3000` | Public URL for the "Open Grafana" link. This one IS exposed to the browser (`NEXT_PUBLIC_*` convention), since it's just a link, not a credential. When fronted by the Caddy proxy in `compose.snippet.yml`, set this to `http://<host>:<published-port>/grafana` instead (currently `7231` in the top-level `compose.yml`). **Note:** `app/page.tsx` is statically prerendered, so this must be passed as a Docker build arg (see `Dockerfile`), not just a runtime env var — setting it under compose's `environment:` alone has no effect on an already-built image. |
| `PORT` | `3000` | Port the standalone server listens on (set by the Dockerfile). |
| `HOSTNAME` | `0.0.0.0` | Bind address (set by the Dockerfile). |

None of the ARI_* variables are `NEXT_PUBLIC_*`, so they're only readable in
route handlers / server code (`lib/ari.ts`, `lib/ari-events.ts`, everything
under `app/api/**`) — the browser only ever talks to this app's own `/api/*`
routes.

## Architecture

- **BFF layer** (`lib/ari.ts`): a thin `fetch`-based helper for ARI REST
  calls with a 5s timeout and basic auth, plus `ariWebSocketUrl()` which
  derives the `ws://.../ari/events` URL for the events subscription.
- **Route handlers** (`app/api/*/route.ts`):
  - `GET /api/health` → `/ari/asterisk/info`, normalized to
    `{ reachable, version, startupTime, ... }`. Always returns HTTP 200 —
    `reachable: false` is how "Asterisk isn't up yet" is represented, not a
    5xx.
  - `GET /api/channels` → `/ari/channels`.
  - `GET /api/endpoints` → `/ari/endpoints`.
  - `GET /api/events` → Server-Sent Events. Backed by a module-level
    singleton (`lib/ari-events.ts`) that opens exactly one upstream
    WebSocket to ARI's `/ari/events?app=aistack-portal&subscribeAll=true`
    on the first SSE subscriber, fans every event out to all connected
    browser tabs, and reconnects with exponential backoff (1s → 30s cap) if
    the upstream socket drops. The connection is closed again once the last
    subscriber disconnects.
- **UI** (`app/page.tsx` + `components/Dashboard.tsx`): a client component
  that hydrates from the three REST routes on mount, opens an `EventSource`
  to `/api/events`, and on each inbound ARI event does a debounced re-fetch
  of channels/endpoints (simple "resync on any event" strategy rather than
  hand-diffing every ARI event type). A 15s poll runs alongside SSE as a
  safety net in case an intermediary proxy buffers or drops the stream.

## Fitting into the compose stack

This app is designed to run as the `portal` service in the docker-compose
stack, on the shared `aistack` bridge network, talking to `asterisk:8088`
and linking out to `grafana:3000` (or `/grafana` once proxied). See:

- `Dockerfile` — multi-stage build (`deps` → `build` → `runtime`), runtime
  stage is `node:22-alpine` running `.next/standalone/server.js` as a
  non-root user, `EXPOSE 3000`.
- `compose.snippet.yml` — the `portal` and `caddy` service blocks to merge
  into the top-level `compose.yml` (not merged automatically — this repo's
  `compose.yml` is owned by another workstream). Caddy is the only service
  meant to publish a host port (`7231:80` in the top-level `compose.yml`);
  everything else, including this portal, stays internal to `aistack`.
- `caddy/Caddyfile` — routes `/` to the portal, `/grafana/*` to Grafana
  (sub-path mode — see the env vars noted in the Caddyfile comments and
  `compose.snippet.yml`), and `/ws`, `/media/*`, `/ari/*`, `/metrics` to
  Asterisk's HTTP server, with native WebSocket upgrade support.

This app itself never opens a host port — it's only reachable through the
edge proxy (or directly by service name from other containers on
`aistack`).

## Local development

```sh
npm install
npm run dev      # http://localhost:3000, expects ARI reachable at ARI_BASE_URL
```

Without a real Asterisk to point at, run with defaults and the dashboard
will simply show "Unreachable" — every route degrades gracefully rather than
erroring.

## Production build

```sh
npm install
npm run build     # next build, output: "standalone"
PORT=3100 npm run start
```

Or via Docker:

```sh
docker build -t asterisk-portal ./portal
docker run --rm -p 3100:3000 \
  -e ARI_BASE_URL=http://asterisk:8088/ari \
  -e ARI_USERNAME=aistack \
  -e ARI_PASSWORD=changeme-ari \
  asterisk-portal
```

## Phase 2 ideas

- **Write actions**: originate/hangup/mute/hold channels, redirect calls
  into a Stasis app, kick/pause queue members — all via ARI POST/DELETE
  routes proxied the same way as the read-only GETs, with an auth layer in
  front of the portal itself (this phase has none — it's read-only and
  assumed to sit behind the trusted `aistack` network / Caddy).
- **Auth**: the portal has no login of its own yet; add one (or front it
  with an auth proxy) before exposing it beyond a trusted network.
- **PJSIP config visibility**: surface `pjsip.conf`/`aor`/`auth` sections
  (via ARI's `/ari/asterisk/config` endpoints or a read of the mounted
  config) so endpoint state can be cross-referenced with its config.
- **CDR / call history**: a paginated view over CDRs (would need a CDR
  backend — e.g. `cdr_pgsql` — exposed via its own small API, since ARI
  itself doesn't serve historical records).
- **Per-channel detail drawer**: click a channel row for its full variables,
  bridge membership, and RTP stats.
- **Queues**: `/ari/queues` — live queue depth, agent state.
- **Richer event handling**: replace the "any event → resync" debounce with
  actual incremental state updates (`ChannelCreated`/`ChannelDestroyed`/
  `ChannelStateChange` etc. mutating the in-memory list directly) once the
  event volume justifies it.
- **Auth for `/metrics` and Grafana dashboards**: provision a default
  Grafana dashboard for the Asterisk Prometheus metrics and deep-link to it
  instead of just Grafana's home page.
