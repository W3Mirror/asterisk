import { NextResponse } from "next/server";
import { AriError, ariGet, type AriEndpoint } from "@/lib/ari";

export const dynamic = "force-dynamic";

export async function GET() {
  try {
    const endpoints = await ariGet<AriEndpoint[]>("/endpoints");
    return NextResponse.json({ endpoints });
  } catch (err) {
    const message = err instanceof AriError ? err.message : "Unknown error contacting Asterisk";
    return NextResponse.json({ endpoints: [], error: message }, { status: 200 });
  }
}
