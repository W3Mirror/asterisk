import { NextResponse } from "next/server";
import { AriError, ariConfig, ariGet, type AriInfo } from "@/lib/ari";

export const dynamic = "force-dynamic";

export async function GET() {
  try {
    const info = await ariGet<AriInfo>("/asterisk/info");

    return NextResponse.json({
      reachable: true,
      version: info.system?.version ?? "unknown",
      startupTime: info.status?.startup_time ?? null,
      lastReloadTime: info.status?.last_reload_time ?? null,
      entityId: info.system?.entity_id ?? null,
      ariBaseUrl: ariConfig.baseUrl,
    });
  } catch (err) {
    const message = err instanceof AriError ? err.message : "Unknown error contacting Asterisk";
    // Reachability failure is an expected, normal state (e.g. Asterisk not
    // running yet) — respond 200 with reachable:false rather than 5xx so the
    // dashboard can render a clean "disconnected" state instead of an error.
    return NextResponse.json({
      reachable: false,
      version: null,
      startupTime: null,
      lastReloadTime: null,
      entityId: null,
      ariBaseUrl: ariConfig.baseUrl,
      error: message,
    });
  }
}
