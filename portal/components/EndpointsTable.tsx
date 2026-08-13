"use client";

import type { AriEndpoint } from "@/lib/ari";

function pillClassFor(state: string): string {
  const s = state.toLowerCase();
  if (s === "online") return "pill up";
  if (s === "offline") return "pill down";
  if (s === "unknown") return "pill";
  return "pill unavailable";
}

export default function EndpointsTable({ endpoints }: { endpoints: AriEndpoint[] }) {
  return (
    <section className="panel">
      <div className="panel-header">
        <h2>Endpoints</h2>
        <span className="count">{endpoints.length}</span>
      </div>
      <table>
        <thead>
          <tr>
            <th>Resource</th>
            <th>Technology</th>
            <th>State</th>
            <th>Channels</th>
          </tr>
        </thead>
        <tbody>
          {endpoints.length === 0 ? (
            <tr className="empty-row">
              <td colSpan={4}>No endpoints configured</td>
            </tr>
          ) : (
            endpoints.map((endpoint) => (
              <tr key={`${endpoint.technology}/${endpoint.resource}`}>
                <td className="mono">{endpoint.resource}</td>
                <td>{endpoint.technology}</td>
                <td>
                  <span className={pillClassFor(endpoint.state)}>{endpoint.state}</span>
                </td>
                <td>{endpoint.channel_ids?.length ?? 0}</td>
              </tr>
            ))
          )}
        </tbody>
      </table>
    </section>
  );
}
