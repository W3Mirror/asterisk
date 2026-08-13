"use client";

import type { AriChannel } from "@/lib/ari";

function techOf(name: string): string {
  const idx = name.indexOf("/");
  return idx === -1 ? name : name.slice(0, idx);
}

function callerIdOf(channel: AriChannel): string {
  const num = channel.caller?.number?.trim();
  const name = channel.caller?.name?.trim();
  if (name && num) return `"${name}" <${num}>`;
  if (num) return num;
  if (name) return name;
  return "-";
}

function formatDuration(creationTime: string | undefined, now: number): string {
  if (!creationTime) return "-";
  const start = new Date(creationTime).getTime();
  if (Number.isNaN(start)) return "-";
  const seconds = Math.max(0, Math.floor((now - start) / 1000));
  const minutes = Math.floor(seconds / 60);
  const secs = seconds % 60;
  const hours = Math.floor(minutes / 60);
  if (hours > 0) {
    return `${hours}:${String(minutes % 60).padStart(2, "0")}:${String(secs).padStart(2, "0")}`;
  }
  return `${minutes}:${String(secs).padStart(2, "0")}`;
}

export default function ChannelsTable({ channels, now }: { channels: AriChannel[]; now: number }) {
  return (
    <section className="panel">
      <div className="panel-header">
        <h2>Active Channels</h2>
        <span className="count">{channels.length}</span>
      </div>
      <table>
        <thead>
          <tr>
            <th>Name</th>
            <th>Tech</th>
            <th>State</th>
            <th>Caller ID</th>
            <th>Duration</th>
          </tr>
        </thead>
        <tbody>
          {channels.length === 0 ? (
            <tr className="empty-row">
              <td colSpan={5}>No active channels</td>
            </tr>
          ) : (
            channels.map((channel) => (
              <tr key={channel.id}>
                <td className="mono" title={channel.name}>
                  {channel.name}
                </td>
                <td>{techOf(channel.name)}</td>
                <td>
                  <span className="pill">{channel.state}</span>
                </td>
                <td>{callerIdOf(channel)}</td>
                <td className="mono">{formatDuration(channel.creationtime, now)}</td>
              </tr>
            ))
          )}
        </tbody>
      </table>
    </section>
  );
}
