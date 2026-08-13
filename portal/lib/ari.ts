/**
 * Server-only helper for talking to the Asterisk REST Interface (ARI).
 *
 * Credentials are read from the environment and never sent to the browser —
 * every route handler under app/api/** proxies through here instead of the
 * client hitting Asterisk directly.
 */

const ARI_BASE_URL = process.env.ARI_BASE_URL ?? "http://asterisk:8088/ari";
const ARI_USERNAME = process.env.ARI_USERNAME ?? "aistack";
const ARI_PASSWORD = process.env.ARI_PASSWORD ?? "changeme-ari";

/** Derives the ARI events WebSocket URL from ARI_BASE_URL (http -> ws, https -> wss). */
export function ariWebSocketUrl(app: string): string {
  const httpUrl = new URL(ARI_BASE_URL);
  const wsProtocol = httpUrl.protocol === "https:" ? "wss:" : "ws:";
  const wsUrl = new URL(`${httpUrl.pathname.replace(/\/$/, "")}/events`, httpUrl);
  wsUrl.protocol = wsProtocol;
  wsUrl.searchParams.set("app", app);
  wsUrl.searchParams.set("subscribeAll", "true");
  wsUrl.searchParams.set("api_key", `${ARI_USERNAME}:${ARI_PASSWORD}`);
  return wsUrl.toString();
}

export class AriError extends Error {
  status?: number;

  constructor(message: string, status?: number) {
    super(message);
    this.name = "AriError";
    this.status = status;
  }
}

function authHeader(): string {
  const token = Buffer.from(`${ARI_USERNAME}:${ARI_PASSWORD}`).toString("base64");
  return `Basic ${token}`;
}

/**
 * Performs a GET request against the ARI REST API and returns the parsed JSON body.
 * Throws AriError on network failure or non-2xx response so callers can decide
 * how to degrade gracefully (e.g. render "unreachable" instead of crashing).
 */
export async function ariGet<T>(path: string, timeoutMs = 5000): Promise<T> {
  const url = `${ARI_BASE_URL.replace(/\/$/, "")}${path}`;
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);

  try {
    const res = await fetch(url, {
      headers: {
        Authorization: authHeader(),
        Accept: "application/json",
      },
      signal: controller.signal,
      cache: "no-store",
    });

    if (!res.ok) {
      throw new AriError(`ARI request to ${path} failed: ${res.status} ${res.statusText}`, res.status);
    }

    return (await res.json()) as T;
  } catch (err) {
    if (err instanceof AriError) throw err;
    const message = err instanceof Error ? err.message : String(err);
    throw new AriError(`ARI request to ${path} failed: ${message}`);
  } finally {
    clearTimeout(timer);
  }
}

export const ariConfig = {
  baseUrl: ARI_BASE_URL,
  username: ARI_USERNAME,
};

// ---- Shared ARI shape helpers (subset of fields the UI actually uses) ----

export interface AriChannel {
  id: string;
  name: string;
  state: string;
  caller?: { name?: string; number?: string };
  connected?: { name?: string; number?: string };
  creationtime?: string;
  dialplan?: { context?: string; exten?: string; priority?: number };
  channelvars?: Record<string, string>;
}

export interface AriEndpoint {
  technology: string;
  resource: string;
  state: string;
  channel_ids: string[];
}

export interface AriInfo {
  build?: {
    os?: string;
    kernel?: string;
    machine?: string;
    date?: string;
  };
  system?: {
    version?: string;
    entity_id?: string;
  };
  status?: {
    startup_time?: string;
    last_reload_time?: string;
  };
}
