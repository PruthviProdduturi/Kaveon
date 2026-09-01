import { API_BASE } from "../config";

/**
 * Authenticated fetch to kaveon-api — routed through the same-origin NextAuth proxy
 * (/api/kaveon/...). The proxy reads the NextAuth session server-side and injects
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
			? `/api/kaveon${path}`
			: input;

	return fetch(target, { ...init, credentials: "include", cache: "no-store" });
}

/**
 * msalFetch that retries transient 5xx / network failures with a short backoff.
 * Use ONLY for idempotent reads (e.g. SQL SELECT execution, chart data) — the
 * backend already retries stale DB connections, but a request can still surface a
 * transient 500 if the pool blips mid-flight; retrying smooths that over so a
 * dashboard tile doesn't show a hard error for a blip that succeeds on retry.
 */
export async function msalFetchRetry(
	input: RequestInfo,
	init: RequestInit = {},
	opts: { retries?: number; backoffMs?: number } = {},
): Promise<Response> {
	const retries = opts.retries ?? 2;
	const backoffMs = opts.backoffMs ?? 250;
	let lastErr: unknown;
	for (let attempt = 0; attempt <= retries; attempt++) {
		try {
			const res = await msalFetch(input, init);
			// Retry only on transient server errors; 4xx are the caller's problem.
			if (res.status >= 500 && res.status <= 504 && attempt < retries) {
				await new Promise((r) => setTimeout(r, backoffMs * (attempt + 1)));
				continue;
			}
			return res;
		} catch (e) {
			// Network-level failure (dropped connection, etc.) — retry with backoff.
			lastErr = e;
			if (attempt < retries) {
				await new Promise((r) => setTimeout(r, backoffMs * (attempt + 1)));
				continue;
			}
			throw lastErr;
		}
	}
	// Exhausted retries on 5xx — return the last response by doing one final call.
	return msalFetch(input, init);
}

/**
 * Deprecated: tokens are no longer handled client-side (NextAuth session cookie
 * + server-side proxy). Kept as a no-op so any lingering imports don't break.
 */
export async function getAccessToken(): Promise<string> {
	return "";
}
