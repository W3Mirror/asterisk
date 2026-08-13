"use client";

export interface HealthState {
  reachable: boolean;
  version: string | null;
  startupTime: string | null;
  lastReloadTime: string | null;
  entityId: string | null;
  ariBaseUrl?: string;
  error?: string;
}

function formatUptime(startupTime: string | null): string {
  if (!startupTime) return "-";
  const start = new Date(startupTime).getTime();
  if (Number.isNaN(start)) return "-";
  const seconds = Math.max(0, Math.floor((Date.now() - start) / 1000));

  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const secs = seconds % 60;

  const parts = [];
  if (days) parts.push(`${days}d`);
  if (days || hours) parts.push(`${hours}h`);
  if (days || hours || minutes) parts.push(`${minutes}m`);
  parts.push(`${secs}s`);
  return parts.join(" ");
}

export default function HealthCard({
  health,
  sseConnected,
  now,
}: {
  health: HealthState | null;
  sseConnected: boolean;
  now: number;
}) {
  // `now` is passed in from a ticking parent so uptime re-renders every
  // second without this component owning its own timer.
  void now;

  const reachable = health?.reachable ?? false;

  return (
    <div className="cards">
      <div className="card">
        <div className="label">Asterisk</div>
        <div className="value">
          <span className={`status-dot ${reachable ? "green" : "red"}`} />
          {reachable ? "Connected" : "Unreachable"}
        </div>
      </div>

      <div className="card">
        <div className="label">Version</div>
        <div className="value mono">{health?.version ?? "-"}</div>
      </div>

      <div className="card">
        <div className="label">Uptime</div>
        <div className="value mono">{reachable ? formatUptime(health?.startupTime ?? null) : "-"}</div>
      </div>

      <div className="card">
        <div className="label">Live event stream</div>
        <div className="value">
          <span className={`status-dot ${sseConnected ? "green" : "amber"}`} />
          {sseConnected ? "Streaming" : "Connecting"}
        </div>
      </div>
    </div>
  );
}
