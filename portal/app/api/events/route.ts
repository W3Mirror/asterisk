import { ariEventBus } from "@/lib/ari-events";

// The 'ws' package needs real Node sockets, so this route cannot run on the
// Edge runtime.
export const runtime = "nodejs";
export const dynamic = "force-dynamic";

const encoder = new TextEncoder();
const KEEPALIVE_MS = 25_000;

export async function GET(request: Request) {
  let unsubscribe: (() => void) | null = null;
  let keepalive: ReturnType<typeof setInterval> | null = null;

  const stream = new ReadableStream<Uint8Array>({
    start(controller) {
      const send = (payload: string) => {
        controller.enqueue(encoder.encode(`data: ${payload}\n\n`));
      };

      // Greet the client immediately so it can render connection state
      // without waiting on the first upstream ARI event.
      send(
        JSON.stringify({
          type: "_portal_status",
          status: ariEventBus.status,
          at: new Date().toISOString(),
        })
      );

      unsubscribe = ariEventBus.subscribe(send);

      // SSE connections idle through proxies (e.g. Caddy) that may time out
      // a connection with no traffic; a comment-only ping keeps it alive.
      keepalive = setInterval(() => {
        try {
          controller.enqueue(encoder.encode(`: keepalive\n\n`));
        } catch {
          // stream already closed
        }
      }, KEEPALIVE_MS);

      request.signal.addEventListener("abort", () => {
        if (keepalive) clearInterval(keepalive);
        if (unsubscribe) unsubscribe();
        try {
          controller.close();
        } catch {
          // already closed
        }
      });
    },
    cancel() {
      if (keepalive) clearInterval(keepalive);
      if (unsubscribe) unsubscribe();
    },
  });

  return new Response(stream, {
    headers: {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache, no-transform",
      Connection: "keep-alive",
      "X-Accel-Buffering": "no",
    },
  });
}
