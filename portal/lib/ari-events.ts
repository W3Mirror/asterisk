/**
 * Singleton fan-out manager between the Asterisk ARI events WebSocket and any
 * number of browser-side SSE subscribers.
 *
 * Only one upstream WebSocket connection to Asterisk is ever open at a time
 * (opened lazily on the first SSE subscriber), and every connected browser
 * tab receives the same event stream. On disconnect the manager reconnects
 * with exponential backoff for as long as at least one subscriber remains.
 */
import WebSocket from "ws";
import { ariWebSocketUrl } from "./ari";

const ARI_APP_NAME = "aistack-portal";
const MAX_BACKOFF_MS = 30_000;
const INITIAL_BACKOFF_MS = 1000;

type Subscriber = {
  id: number;
  send: (payload: string) => void;
};

class AriEventBus {
  private subscribers = new Map<number, Subscriber>();
  private nextId = 1;
  private ws: WebSocket | null = null;
  private backoffMs = INITIAL_BACKOFF_MS;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private closedByUs = false;

  /** Current upstream connection state, exposed for diagnostics/health. */
  status: "disconnected" | "connecting" | "connected" = "disconnected";

  subscribe(send: (payload: string) => void): () => void {
    const id = this.nextId++;
    this.subscribers.set(id, { id, send });

    if (this.subscribers.size === 1) {
      this.ensureConnected();
    }

    return () => {
      this.subscribers.delete(id);
      // Keep the upstream connection warm even with zero subscribers for a
      // short grace period would be nicer, but closing immediately is
      // simplest and cheapest — it reopens instantly on the next subscriber.
      if (this.subscribers.size === 0) {
        this.disconnect();
      }
    };
  }

  private broadcast(data: string) {
    for (const sub of this.subscribers.values()) {
      try {
        sub.send(data);
      } catch {
        // Ignore individual client write failures; the SSE route's own
        // cleanup (abort signal) will drop the subscriber.
      }
    }
  }

  private ensureConnected() {
    if (this.ws || this.reconnectTimer) return;
    this.closedByUs = false;
    this.status = "connecting";

    const url = ariWebSocketUrl(ARI_APP_NAME);
    let socket: WebSocket;
    try {
      socket = new WebSocket(url);
    } catch {
      this.scheduleReconnect();
      return;
    }
    this.ws = socket;

    socket.on("open", () => {
      this.status = "connected";
      this.backoffMs = INITIAL_BACKOFF_MS;
      this.broadcast(
        JSON.stringify({ type: "_portal_status", status: "connected", at: new Date().toISOString() })
      );
    });

    socket.on("message", (data) => {
      // ARI sends one JSON event object per WS message; forward as-is.
      this.broadcast(data.toString());
    });

    socket.on("error", () => {
      // 'close' fires right after; reconnection is handled there.
    });

    socket.on("close", () => {
      this.ws = null;
      this.status = "disconnected";
      if (!this.closedByUs && this.subscribers.size > 0) {
        this.broadcast(
          JSON.stringify({ type: "_portal_status", status: "disconnected", at: new Date().toISOString() })
        );
        this.scheduleReconnect();
      }
    });
  }

  private scheduleReconnect() {
    if (this.reconnectTimer) return;
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      if (this.subscribers.size > 0) {
        this.ensureConnected();
      }
    }, this.backoffMs);
    this.backoffMs = Math.min(this.backoffMs * 2, MAX_BACKOFF_MS);
  }

  private disconnect() {
    this.closedByUs = true;
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    if (this.ws) {
      this.ws.removeAllListeners();
      try {
        this.ws.close();
      } catch {
        // already closed
      }
      this.ws = null;
    }
    this.status = "disconnected";
    this.backoffMs = INITIAL_BACKOFF_MS;
  }
}

// A module-level singleton: Next.js keeps route handler modules resident for
// the lifetime of the server process, so this survives across requests and
// is shared by every SSE connection.
const globalForAriBus = globalThis as unknown as { __ariEventBus?: AriEventBus };

export const ariEventBus = globalForAriBus.__ariEventBus ?? new AriEventBus();
globalForAriBus.__ariEventBus = ariEventBus;
