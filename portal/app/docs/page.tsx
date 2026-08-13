import type { Metadata } from "next";

export const metadata: Metadata = {
  title: "Docs · Asterisk Portal",
  description: "How to use this stack as a SIP server",
};

const TOC = [
  { href: "#overview", label: "1. Overview" },
  { href: "#endpoints", label: "2. Endpoints" },
  { href: "#registering", label: "3. Registering a SIP client" },
  { href: "#configuring-endpoints", label: "4. Configuring endpoints" },
  { href: "#dialplan", label: "5. Dialplan" },
  { href: "#chan-websocket", label: "6. chan_websocket usage" },
  { href: "#ari", label: "7. ARI" },
  { href: "#security", label: "8. Security checklist" },
];

export default function DocsPage() {
  return (
    <main className="page">
      <div className="page-header">
        <div>
          <h1>Documentation</h1>
          <div className="subtitle">Using this stack as a SIP server</div>
        </div>
        <a className="grafana-link" href="/">
          &larr; Back to dashboard
        </a>
      </div>

      <nav className="docs-toc" aria-label="Table of contents">
        {TOC.map((item) => (
          <a key={item.href} href={item.href}>
            {item.label}
          </a>
        ))}
      </nav>

      <div className="docs-content">
        <section id="overview">
          <h2>1. Overview</h2>
          <p>
            This stack is a self-contained Asterisk 22.10.1 PBX fronted by a
            single Caddy edge proxy. Five containers share one Docker network
            (<code>aistack</code>), and only Caddy publishes a port to the
            host &mdash; everything else (Asterisk&apos;s HTTP server, the
            portal, Grafana, Prometheus) is reached by service name over that
            internal network.
          </p>
          <ul>
            <li>
              <strong>asterisk</strong> &mdash; Asterisk 22.10.1. SIP
              signalling and media both ride over WebSocket, on the built-in
              HTTP server (port 8088 internally): PJSIP&apos;s{" "}
              <code>transport-ws</code> for SIP-over-WebSocket, the native{" "}
              <code>chan_websocket</code> driver for per-call media, ARI for
              REST/event control, and <code>res_prometheus</code> for
              metrics.
            </li>
            <li>
              <strong>caddy</strong> &mdash; the only published port
              (<code>7231</code> &rarr; container port <code>80</code>).
              Routes by path to the portal, Grafana, and Asterisk&apos;s HTTP
              server, transparently upgrading the WebSocket routes. See{" "}
              <code>docker/caddy/Caddyfile</code>.
            </li>
            <li>
              <strong>portal</strong> &mdash; this Next.js app. A read-only
              dashboard (live channels/endpoints/health via ARI) plus this
              docs page. Never exposes a host port; only reachable through
              Caddy.
            </li>
            <li>
              <strong>grafana</strong> &mdash; dashboards over Asterisk
              metrics, mounted at <code>/grafana/</code> (Grafana&apos;s
              sub-path mode).
            </li>
            <li>
              <strong>prometheus</strong> &mdash; scrapes{" "}
              <code>http://asterisk:8088/metrics</code> on the internal
              network; not published to the host at all.
            </li>
          </ul>
          <p>
            Everything (SIP signalling, per-call media, ARI, metrics) is
            reachable through <code>:7231</code> over plain WebSocket/HTTP by
            default &mdash; there&apos;s no separate SIP or RTP port exposed
            unless you opt in (see{" "}
            <a href="#registering">Registering a SIP client</a>).
          </p>
        </section>

        <section id="endpoints">
          <h2>2. Endpoints</h2>
          <p>
            All paths below are served through Caddy at{" "}
            <code>http://100.69.165.55:7231</code> (or{" "}
            <code>http://localhost:7231</code> on the host itself).
          </p>
          <section className="panel">
            <table>
              <thead>
                <tr>
                  <th>Path</th>
                  <th>Backend</th>
                  <th>What it is</th>
                </tr>
              </thead>
              <tbody>
                <tr>
                  <td className="mono">/</td>
                  <td>portal</td>
                  <td>Live dashboard &mdash; channels, endpoints, health.</td>
                </tr>
                <tr>
                  <td className="mono">/docs</td>
                  <td>portal</td>
                  <td>This page.</td>
                </tr>
                <tr>
                  <td className="mono">/grafana/</td>
                  <td>grafana</td>
                  <td>
                    Grafana UI, sub-path mode. Bare <code>/grafana</code>{" "}
                    (no trailing slash) 308-redirects here.
                  </td>
                </tr>
                <tr>
                  <td className="mono">/ws</td>
                  <td>asterisk:8088</td>
                  <td>
                    SIP over WebSocket signalling (PJSIP{" "}
                    <code>transport-ws</code>). WebSocket subprotocol{" "}
                    <code>sip</code>.
                  </td>
                </tr>
                <tr>
                  <td className="mono">/media/&lt;connection_id&gt;</td>
                  <td>asterisk:8088</td>
                  <td>
                    Per-call media/control socket (<code>chan_websocket</code>
                    ). WebSocket subprotocol <code>media</code>.
                  </td>
                </tr>
                <tr>
                  <td className="mono">/ari/*</td>
                  <td>asterisk:8088</td>
                  <td>
                    Asterisk REST Interface, HTTP basic auth. Includes the
                    events websocket at <code>/ari/events</code>.
                  </td>
                </tr>
                <tr>
                  <td className="mono">/metrics</td>
                  <td>asterisk:8088</td>
                  <td>Prometheus text-format metrics (res_prometheus).</td>
                </tr>
              </tbody>
            </table>
          </section>
        </section>

        <section id="registering">
          <h2>3. Registering a SIP client</h2>
          <p>
            A demo endpoint is preconfigured in{" "}
            <code>docker/etc-asterisk/pjsip.conf</code>:
          </p>
          <pre>
            <code>{`[6001]
type=endpoint
context=from-internal
disallow=all
allow=ulaw
allow=alaw
auth=6001
aors=6001
direct_media=no

[6001]
type=auth
auth_type=userpass
username=6001
password=changeme6001

[6001]
type=aor
max_contacts=5
remove_existing=yes`}</code>
          </pre>
          <p>
            Two transports are defined: <code>transport-ws</code> (SIP over
            WebSocket, rides on the HTTP server / port 8088, reached through
            Caddy at <code>/ws</code>) and <code>transport-udp</code>{" "}
            (classic UDP SIP, bound to <code>0.0.0.0:5060</code> inside the
            container &mdash; see the note on UDP/RTP below).
          </p>
          <h3>Browser client (JsSIP)</h3>
          <pre>
            <code>{`const ua = new JsSIP.UA({
  sockets: [new JsSIP.WebSocketInterface('ws://100.69.165.55:7231/ws')],
  uri: 'sip:6001@100.69.165.55',
  authorization_user: '6001',
  password: 'changeme6001',
  register: true,
});
ua.start();`}</code>
          </pre>
          <h3>Browser client (SIP.js)</h3>
          <pre>
            <code>{`const userAgent = new SIP.UserAgent({
  uri: SIP.UserAgent.makeURI('sip:6001@100.69.165.55'),
  transportOptions: { server: 'ws://100.69.165.55:7231/ws' },
  authorizationUsername: '6001',
  authorizationPassword: 'changeme6001',
});
userAgent.start().then(() => {
  const registerer = new SIP.Registerer(userAgent);
  registerer.register();
});`}</code>
          </pre>
          <div className="callout">
            <strong>Note:</strong> the examples above use plain{" "}
            <code>ws://</code> to match the stack&apos;s default (no TLS
            configured out of the box &mdash; see{" "}
            <code>docker/etc-asterisk/http.conf</code>). For anything beyond
            local dev/demo, terminate TLS at Caddy (or enable Asterisk&apos;s{" "}
            <code>tlsenable</code>/<code>tlsbindaddr</code> in{" "}
            <code>http.conf</code>) and use <code>wss://</code> instead.
          </div>
          <h3>UDP/RTP clients (hardware phones, trunks)</h3>
          <p>
            Plain UDP SIP (port 5060) and RTP (ports 10000&ndash;10100, set in{" "}
            <code>docker/etc-asterisk/rtp.conf</code>) are{" "}
            <strong>not published to the host by default</strong>. To use
            them, uncomment the commented <code>ports:</code> block under the{" "}
            <code>asterisk</code> service in <code>compose.yml</code>:
          </p>
          <pre>
            <code>{`# ports:
#   - "5060:5060/udp"
#   - "10000-10100:10000-10100/udp"`}</code>
          </pre>
          <p>
            then <code>docker compose up -d asterisk</code> to recreate the
            container with those ports exposed.
          </p>
        </section>

        <section id="configuring-endpoints">
          <h2>4. Configuring endpoints</h2>
          <p>
            Add new <code>endpoint</code>/<code>auth</code>/<code>aor</code>{" "}
            sections to <code>docker/etc-asterisk/pjsip.conf</code>, following
            the <code>6001</code> block above as a template (give the new
            endpoint its own section name, e.g. <code>[6002]</code>, its own
            password, and reference it from <code>auth=</code>/
            <code>aors=</code>). The file is bind-mounted read-only into the
            container at <code>/etc/asterisk</code>, so edits on the host are
            visible inside the container immediately &mdash; Asterisk just
            needs to be told to re-read it:
          </p>
          <pre>
            <code>{`docker compose exec asterisk asterisk -rx "module reload res_pjsip.so"`}</code>
          </pre>
          <p>
            (There is no standalone <code>pjsip reload</code> CLI command in
            this build &mdash; only narrower ones like{" "}
            <code>pjsip reload qualify endpoint</code>. A full config reload
            goes through the module reload above.) Confirm it picked up the
            change with:
          </p>
          <pre>
            <code>{`docker compose exec asterisk asterisk -rx "pjsip show endpoints"`}</code>
          </pre>
        </section>

        <section id="dialplan">
          <h2>5. Dialplan</h2>
          <p>
            <code>docker/etc-asterisk/extensions.conf</code> defines a single
            context, <code>from-internal</code>, used by every demo endpoint:
          </p>
          <pre>
            <code>{`[from-internal]
exten => 100,1,Answer()
 same => n,Playback(hello-world)
 same => n,Hangup()

;exten => 200,1,Answer()
; same => n,Dial(WebSocket/INCOMING/c(ulaw))
; same => n,Hangup()`}</code>
          </pre>
          <ul>
            <li>
              <strong>Extension 100</strong> is the built-in test: answers and
              plays the <code>hello-world</code> announcement, confirming
              audio/RTP and the PJSIP endpoint are working end to end. Dial it
              from 6001 (or any registered endpoint) to sanity-check a new
              client.
            </li>
            <li>
              <strong>Extension 200</strong> (commented out) is a template for
              bridging a call to a <code>chan_websocket</code> client &mdash;
              see <a href="#chan-websocket">chan_websocket usage</a> below.
            </li>
          </ul>
          <p>
            After editing <code>extensions.conf</code>, reload the dialplan
            without restarting Asterisk:
          </p>
          <pre>
            <code>{`docker compose exec asterisk asterisk -rx "dialplan reload"`}</code>
          </pre>
        </section>

        <section id="chan-websocket">
          <h2>6. chan_websocket usage</h2>
          <p>
            <code>chan_websocket</code> is a separate, native WebSocket
            channel driver &mdash; distinct from PJSIP&apos;s{" "}
            <code>transport-ws</code> signalling transport. It carries a
            call&apos;s media/control frames directly over a WebSocket,
            independent of SIP. Dial string:
          </p>
          <pre>
            <code>{`Dial(WebSocket/<connection_id>[/options])`}</code>
          </pre>
          <p>
            Use the literal connection id <code>INCOMING</code> to wait for
            an inbound WebSocket connection rather than opening an outbound
            one:
          </p>
          <pre>
            <code>{`exten => 200,1,Answer()
 same => n,Dial(WebSocket/INCOMING/c(ulaw))
 same => n,Hangup()`}</code>
          </pre>
          <p>
            When an incoming connection is used, Asterisk generates a random
            connection id for that call and exposes it on the channel as the{" "}
            <code>MEDIA_WEBSOCKET_CONNECTION_ID</code> variable. Your external
            client then opens a WebSocket to{" "}
            <code>/media/&lt;connection_id&gt;</code> (through Caddy:{" "}
            <code>ws://100.69.165.55:7231/media/&lt;connection_id&gt;</code>)
            with the subprotocol <code>media</code>, and Asterisk bridges it
            in.
          </p>
          <p>Common dial string options:</p>
          <table>
            <thead>
              <tr>
                <th>Option</th>
                <th>Meaning</th>
              </tr>
            </thead>
            <tbody>
              <tr>
                <td className="mono">c(codec)</td>
                <td>
                  Codec to use on the channel (e.g. <code>c(ulaw)</code>).
                  Defaults to the caller channel&apos;s first codec if
                  omitted.
                </td>
              </tr>
              <tr>
                <td className="mono">f(format)</td>
                <td>
                  Control-message format for this call &mdash;{" "}
                  <code>plain-text</code> (default) or <code>json</code>.
                  Overrides the global{" "}
                  <code>control_message_format</code> in{" "}
                  <code>chan_websocket.conf</code> per-call.
                </td>
              </tr>
              <tr>
                <td className="mono">d(direction)</td>
                <td>
                  Media direction from the app&apos;s perspective:{" "}
                  <code>both</code> (default), <code>in</code> (app receives
                  only), or <code>out</code> (app sends only).
                </td>
              </tr>
            </tbody>
          </table>
          <p>
            <code>docker/etc-asterisk/chan_websocket.conf</code> is left at
            its defaults (plain-text control messages); uncomment{" "}
            <code>control_message_format = json</code> under{" "}
            <code>[global]</code> to switch globally instead of per-call.
          </p>
        </section>

        <section id="ari">
          <h2>7. ARI</h2>
          <p>
            The Asterisk REST Interface is enabled with one user,{" "}
            <code>aistack</code>, defined in{" "}
            <code>docker/etc-asterisk/ari.conf</code>. It&apos;s served on
            the same HTTP server as everything else, proxied by Caddy at{" "}
            <code>/ari/*</code>.
          </p>
          <pre>
            <code>{`curl -u aistack:changeme-ari http://100.69.165.55:7231/ari/asterisk/info`}</code>
          </pre>
          <p>
            The events websocket (used by the portal&apos;s live
            channels/endpoints view) is at:
          </p>
          <pre>
            <code>{`ws://100.69.165.55:7231/ari/events?app=<stasis-app-name>&subscribeAll=true`}</code>
          </pre>
          <p>
            with the same basic-auth credentials. See the portal&apos;s own{" "}
            <code>lib/ari.ts</code> / <code>lib/ari-events.ts</code> for a
            worked example (REST calls with a timeout, and a
            reconnecting-with-backoff events subscription fanned out over
            Server-Sent Events).
          </p>
        </section>

        <section id="security">
          <h2>8. Security checklist</h2>
          <p>
            This stack ships with demo credentials meant for local
            dev/testing only. Before exposing it beyond a trusted network,
            at minimum:
          </p>
          <div className="callout">
            <strong>HTTP Basic Auth is enabled at the edge.</strong> Caddy
            (<code>docker/caddy/Caddyfile</code>) protects the portal (
            <code>/</code>, <code>/docs</code>, <code>/api/*</code>),{" "}
            <code>/grafana</code>/<code>/grafana/*</code>, and{" "}
            <code>/metrics</code> with a single <code>admin</code> account
            (bcrypt hash in <code>compose.yml</code>&apos;s{" "}
            <code>CADDY_ADMIN_HASH</code>). <code>/ws</code>,{" "}
            <code>/media/*</code>, and <code>/ari/*</code> are intentionally
            left out of this gate &mdash; SIP/WebRTC clients can&apos;t send
            extra auth headers, and ARI already enforces its own credentials.
            Grafana has <code>GF_AUTH_ANONYMOUS_ENABLED=true</code> (Viewer
            role) so the single Caddy login is enough to view dashboards;
            sign in at <code>/grafana/login</code> for editing.
          </div>
          <div className="callout">
            <strong>Change all three default passwords:</strong>
            <ul>
              <li>
                <code>docker/etc-asterisk/pjsip.conf</code> &mdash; endpoint{" "}
                <code>6001</code>&apos;s <code>password=changeme6001</code>{" "}
                (and any endpoints you add).
              </li>
              <li>
                <code>docker/etc-asterisk/ari.conf</code> &mdash; the{" "}
                <code>aistack</code> user&apos;s{" "}
                <code>password = changeme-ari</code> (also update{" "}
                <code>ARI_PASSWORD</code> in <code>compose.yml</code>&apos;s{" "}
                <code>portal</code> service so the dashboard keeps working).
              </li>
              <li>
                <code>compose.yml</code> &mdash; Grafana&apos;s{" "}
                <code>GF_SECURITY_ADMIN_PASSWORD=changeme</code>.
              </li>
            </ul>
          </div>
          <ul>
            <li>
              <strong>Restrict network exposure.</strong> Caddy is the only
              published port. Consider binding it to a specific interface
              (e.g. the tailscale IP) instead of all interfaces &mdash; change{" "}
              <code>ports: ["7231:80"]</code> to{" "}
              <code>ports: ["100.69.165.55:7231:80"]</code> in{" "}
              <code>compose.yml</code> so the stack is unreachable from other
              interfaces on the host.
            </li>
            <li>
              <strong>Use TLS in production.</strong> Everything here runs
              over plain <code>ws://</code>/<code>http://</code>. Terminate
              TLS at Caddy (it can provision certs automatically for a real
              domain) or enable Asterisk&apos;s own{" "}
              <code>tlsenable</code>/<code>tlsbindaddr</code>/
              <code>tlscertfile</code>/<code>tlsprivatekey</code> in{" "}
              <code>http.conf</code>, and switch clients to{" "}
              <code>wss://</code>.
            </li>
            <li>
              <strong>Lock down <code>/metrics</code>.</strong>{" "}
              <code>docker/etc-asterisk/prometheus.conf</code> notes that
              it&apos;s reachable from outside the docker network (via
              Caddy) with no auth by default; uncomment{" "}
              <code>auth_username</code>/<code>auth_password</code> there
              once Prometheus is updated to send them.
            </li>
            <li>
              <strong>The portal has no login of its own.</strong> It&apos;s
              read-only today, but treat it (and Grafana) as trusted-network
              tools, or put an auth proxy in front, before exposing them
              further.
            </li>
          </ul>
        </section>
      </div>
    </main>
  );
}
