import { NextResponse } from "next/server";
import { configuredEntraPublicClient } from "@/auth/verifyEntra";

export const dynamic = "force-dynamic";
export const revalidate = 0;

const noStore = { headers: { "Cache-Control": "no-store" } };

export function GET() {
  if (process.env.KAVEON_ENTRA_PUBLIC_CLIENT !== "true") return NextResponse.json({ enabled: false }, noStore);
  const config = configuredEntraPublicClient();
  if (!config) return NextResponse.json({ enabled: false }, noStore);
  return NextResponse.json({ enabled: true, clientId: config.clientId, tenantId: config.tenantId, scope: `api://${config.clientId}/access_as_user` }, noStore);
}
