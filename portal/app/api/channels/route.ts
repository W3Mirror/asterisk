import { NextResponse } from "next/server";
import { AriError, ariGet, type AriChannel } from "@/lib/ari";

export const dynamic = "force-dynamic";

export async function GET() {
  try {
    const channels = await ariGet<AriChannel[]>("/channels");
    return NextResponse.json({ channels });
  } catch (err) {
    const message = err instanceof AriError ? err.message : "Unknown error contacting Asterisk";
    // Degrade gracefully: an empty list + error flag, not a 500, so the UI
    // can render "Asterisk unreachable" instead of crashing the page.
    return NextResponse.json({ channels: [], error: message }, { status: 200 });
  }
}
