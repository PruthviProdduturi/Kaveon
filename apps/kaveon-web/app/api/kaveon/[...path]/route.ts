/**
 * kaveon-api proxy — the trusted identity layer (Forge's proxy model).
 *
 * The browser calls this same-origin route (NextAuth session cookie travels
 * automatically). Here, server-side, we read the session with auth() and forward
 * the request to kaveon-api with X-User-* identity headers, stamped with
 * X-Proxy-Secret. kaveon-api trusts those headers only when the secret matches, so
 * the browser can never spoof an identity by hitting kaveon-api directly.
 *
 * Client calls:  /api/lens/api/v1/...   →  {API_URL}/api/v1/...
 */

export const dynamic = "force-dynamic";

import { NextRequest } from "next/server";
import { auth } from "../../../../auth";

const API_BASE = (process.env.API_URL || "http://localhost:8080").replace(/\/+$/, "");
const PROXY_SECRET = process.env.KAVEON_PROXY_SECRET || "";

const DEV_BYPASS = process.env.NODE_ENV === "development" && process.env.KAVEON_DEV_USER_EMAIL;

async function handler(req: NextRequest, ctx: { params: Promise<{ path: string[] }> }) {
	let email = "";
	let name = "";
	let role = "Viewer";

	if (DEV_BYPASS) {
		// Local dev: skip auth() (can hang on Entra OIDC from corporate network)
		email = process.env.KAVEON_DEV_USER_EMAIL!;
		name = process.env.KAVEON_DEV_USER_NAME || "Dev User";
		role = process.env.KAVEON_DEV_USER_ROLE || "Admin";
	} else {
		const session = await auth();
		if (!session?.user) {
			return Response.json({ detail: { code: "unauthorized", message: "Not authenticated" } }, { status: 401 });
		}
		const user = session.user as { email?: string | null; name?: string | null; role?: string };
		email = user.email ?? "";
		name = user.name ?? "";
		role = user.role ?? "Viewer";
	}

	const { path } = await ctx.params;
	const target = `${API_BASE}/${(path ?? []).join("/")}${req.nextUrl.search}`;

	const headers = new Headers(req.headers);
	headers.delete("host");
	headers.delete("content-length");
	headers.set("x-user-email", email);
	headers.set("x-user-name", name);
	headers.set("x-user-role", role);
	headers.set("x-user-roles", role);
	if (PROXY_SECRET) headers.set("x-proxy-secret", PROXY_SECRET);

	const init: RequestInit = { method: req.method, headers, redirect: "manual" };
	if (req.method !== "GET" && req.method !== "HEAD") {
		init.body = await req.arrayBuffer();
	}

	const resp = await fetch(target, init);
	const respHeaders = new Headers(resp.headers);
	respHeaders.delete("content-encoding");
	respHeaders.delete("content-length");
	respHeaders.delete("transfer-encoding");
	return new Response(resp.body, { status: resp.status, headers: respHeaders });
}

export {
	handler as GET,
	handler as POST,
	handler as PUT,
	handler as PATCH,
	handler as DELETE,
	handler as HEAD,
	handler as OPTIONS,
};
