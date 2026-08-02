import { API_BASE } from "../config";

/**
 * Authenticated fetch to lens-api — routed through the same-origin NextAuth proxy
 * (/api/lens/...). The proxy reads the NextAuth session server-side and injects
 * the trusted X-User-* identity headers, so the browser never handles tokens and
 * cannot spoof identity. The session cookie travels via credentials: "include".
 *
 * Named `msalFetch` for backward compatibility with existing call sites; MSAL is
 * no longer used (auth is NextAuth — see auth.ts / auth/useAuth.tsx).
 *
 * Usage: await msalFetch("/api/v1/datasets")  or  msalFetch(`${API_BASE}/api/...`)
 */
export async function msalFetch(input: RequestInfo, init: RequestInit = {}): Promise<Response> {
	let path = typeof input === "string" ? input : "";

	// Normalise: strip an absolute API_BASE prefix so we can route same-origin.
	const base = API_BASE.replace(/\/+$/, "");
	if (path.startsWith(base)) path = path.slice(base.length);

	// Route /api/... calls through the proxy; leave anything else untouched.
	const target =
		typeof input === "string" && path.startsWith("/api")
			? `/api/lens${path}`
			: input;

	return fetch(target, { ...init, credentials: "include" });
}

/**
 * Deprecated: tokens are no longer handled client-side (NextAuth session cookie
 * + server-side proxy). Kept as a no-op so any lingering imports don't break.
 */
export async function getAccessToken(): Promise<string> {
	return "";
}
