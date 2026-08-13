import Dashboard from "@/components/Dashboard";

export default function Home() {
  const grafanaUrl = process.env.NEXT_PUBLIC_GRAFANA_URL ?? "http://localhost:3000";

  return (
    <main className="page">
      <div className="page-header">
        <div>
          <h1>Asterisk Portal</h1>
          <div className="subtitle">Live channels, endpoints &amp; system health</div>
        </div>
        <div className="header-links">
          <a className="grafana-link" href="/docs">
            Docs
          </a>
          <a className="grafana-link" href={grafanaUrl} target="_blank" rel="noreferrer">
            Open Grafana &rarr;
          </a>
        </div>
      </div>

      <Dashboard />

      <footer className="page-footer">
        <span>Phase 1 &middot; read-only</span>
        <span>ARI events via SSE</span>
      </footer>
    </main>
  );
}
