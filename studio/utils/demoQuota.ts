/**
 * The demo posture as the API reports it (`GET /api/v1/engine/quota`): whether
 * the platform is read-only for this user, and their live-read quota on the
 * coordinator. A self-hosted install answers `quota: null` and no counter is
 * shown; the demo cluster answers the standing the Engine enforces.
 */

export interface DemoQuota {
	max_statements: number;
	per_seconds: number;
	count: string;
	resource_group: string;
	used: number;
	remaining: number;
	resets_at?: string;
	resets_at_ms?: number;
	next_allowed_at: string;
	next_allowed_at_ms: number;
}

export interface DemoPosture {
	readOnly: boolean;
	engine: boolean;
	quota: DemoQuota | null;
}

export const RATE_LIMITED = "RATE_LIMITED";
export const DEMO_READ_ONLY = "demo_read_only";

/** `14:20` in the viewer's clock, for a label; `null` for a missing time. */
export function clockOf(iso: string | undefined | null): string | null {
	if (!iso) return null;
	const date = new Date(iso);
	if (Number.isNaN(date.getTime())) return null;
	return date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

/** `3 of 5 live queries left · resets 14:20`; `null` when no quota applies. */
export function quotaLabel(quota: DemoQuota | null): string | null {
	if (!quota) return null;
	const noun = quota.max_statements === 1 ? "live query" : "live queries";
	const head = `${quota.remaining} of ${quota.max_statements} ${noun} left`;
	const when = clockOf(quota.remaining > 0 ? quota.resets_at : quota.next_allowed_at);
	if (!when) return head;
	return quota.remaining > 0 ? `${head} · resets ${when}` : `${head} · next at ${when}`;
}

/** The `detail` of a refused API response, whatever shape the API used. */
function detailOf(body: unknown): { code?: string; message?: string } | string | null {
	if (!body || typeof body !== "object") return null;
	const record = body as { detail?: unknown; error?: unknown };
	const detail = record.detail ?? record.error;
	if (typeof detail === "string") return detail;
	if (detail && typeof detail === "object") return detail as { code?: string; message?: string };
	return null;
}

/** The Engine's quota refusal carried by a 429 body, or `null` when the
 *  response is something else. */
export function rateLimitNotice(status: number, body: unknown): string | null {
	if (status !== 429) return null;
	const detail = detailOf(body);
	if (!detail || typeof detail === "string" || detail.code !== RATE_LIMITED) return null;
	return detail.message || "The demo's live-query quota is used up for now.";
}

/** The read-only refusal carried by a 403 body, or `null`. */
export function readOnlyNotice(status: number, body: unknown): string | null {
	if (status !== 403) return null;
	const detail = detailOf(body);
	if (!detail || typeof detail === "string" || detail.code !== DEMO_READ_ONLY) return null;
	return detail.message || "This demo is read-only.";
}

/** A refusal's message for display, whatever shape the API used. */
export function refusalMessage(body: unknown, fallback: string): string {
	const detail = detailOf(body);
	if (typeof detail === "string") return detail;
	if (detail?.message) return detail.message;
	return fallback;
}

/** An error thrown by an execution path when the Engine refused the
 *  statement on quota: shown as a notice, never as a failure. */
export class RateLimitedError extends Error {
	constructor(message: string) {
		super(message);
		this.name = "RateLimitedError";
	}
}
