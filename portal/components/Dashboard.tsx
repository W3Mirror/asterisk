"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import type { AriChannel, AriEndpoint } from "@/lib/ari";
import HealthCard, { type HealthState } from "./HealthCard";
import ChannelsTable from "./ChannelsTable";
import EndpointsTable from "./EndpointsTable";

const POLL_INTERVAL_MS = 15_000;
const REFRESH_DEBOUNCE_MS = 400;

async function fetchJson<T>(url: string): Promise<T | null> {
  try {
    const res = await fetch(url, { cache: "no-store" });
    if (!res.ok) return null;
    return (await res.json()) as T;
  } catch {
    return null;
  }
}

export default function Dashboard() {
  const [health, setHealth] = useState<HealthState | null>(null);
  const [channels, setChannels] = useState<AriChannel[]>([]);
  const [endpoints, setEndpoints] = useState<AriEndpoint[]>([]);
  const [sseConnected, setSseConnected] = useState(false);
  const [now, setNow] = useState(() => Date.now());
  const [fetchError, setFetchError] = useState<string | null>(null);

  const refreshTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const refreshChannelsAndEndpoints = useCallback(async () => {
    const [channelsRes, endpointsRes] = await Promise.all([
      fetchJson<{ channels: AriChannel[]; error?: string }>("/api/channels"),
      fetchJson<{ endpoints: AriEndpoint[]; error?: string }>("/api/endpoints"),
    ]);
    if (channelsRes) {
      setChannels(channelsRes.channels);
      setFetchError(channelsRes.error ?? null);
    }
    if (endpointsRes) {
      setEndpoints(endpointsRes.endpoints);
    }
  }, []);

  const refreshHealth = useCallback(async () => {
    const healthRes = await fetchJson<HealthState>("/api/health");
    if (healthRes) setHealth(healthRes);
  }, []);

  const scheduleRefresh = useCallback(() => {
    if (refreshTimer.current) clearTimeout(refreshTimer.current);
    refreshTimer.current = setTimeout(() => {
      refreshChannelsAndEndpoints();
    }, REFRESH_DEBOUNCE_MS);
  }, [refreshChannelsAndEndpoints]);

  // Initial hydration from the REST endpoints.
  useEffect(() => {
    refreshHealth();
    refreshChannelsAndEndpoints();
  }, [refreshHealth, refreshChannelsAndEndpoints]);

  // Ticking clock for live duration/uptime displays.
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, []);

  // Periodic poll as a safety net in case SSE is unavailable (e.g. blocked
  // by an intermediary proxy) or an event was missed.
  useEffect(() => {
    const id = setInterval(() => {
      refreshChannelsAndEndpoints();
      refreshHealth();
    }, POLL_INTERVAL_MS);
    return () => clearInterval(id);
  }, [refreshChannelsAndEndpoints, refreshHealth]);

  // Live updates via SSE.
  useEffect(() => {
    const source = new EventSource("/api/events");

    source.onopen = () => setSseConnected(true);
    source.onerror = () => setSseConnected(false);

    source.onmessage = (event) => {
      try {
        const payload = JSON.parse(event.data);
        if (payload?.type === "_portal_status") {
          setSseConnected(payload.status === "connected");
          // A reconnect to ARI can mean we missed events while down —
          // resync immediately.
          if (payload.status === "connected") {
            refreshChannelsAndEndpoints();
            refreshHealth();
          }
          return;
        }
      } catch {
        // Not JSON — ignore.
      }
      // Any real ARI event (ChannelCreated, StasisStart, endpoint state
      // change, etc.) is a cue to resync the tables. Debounced so a burst
      // of events doesn't trigger a fetch storm.
      scheduleRefresh();
    };

    return () => {
      source.close();
      if (refreshTimer.current) clearTimeout(refreshTimer.current);
    };
  }, [refreshChannelsAndEndpoints, refreshHealth, scheduleRefresh]);

  return (
    <>
      <HealthCard health={health} sseConnected={sseConnected} now={now} />

      {fetchError && (
        <div className="error-banner">
          Asterisk is unreachable at the configured ARI endpoint ({fetchError}). Showing last known
          data.
        </div>
      )}

      <ChannelsTable channels={channels} now={now} />
      <EndpointsTable endpoints={endpoints} />
    </>
  );
}
