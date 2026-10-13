/**
 * Role truth for the UI — a direct mirror of `api/middleware/permissions.py`.
 *
 * The API is the authority on who may write what, and it refuses with 403.
 * The UI's job is to not offer an action the API will refuse, so these
 * predicates are written against the API's own rules rather than against a
 * looser idea of them:
 *
 *   create (POST /charts, /dashboards, /datasets)
 *       `require_min_role("Analyst")` — a Viewer never creates.
 *
 *   edit / delete (PUT, PATCH, DELETE on the same three)
 *       `can_write(owner_email, ctx)` — Editor and Admin may touch anyone's
 *       content, an Analyst only its own, and a Viewer nothing at all, not
 *       even content it owns.
 *
 *   publish (`visibility = "published"`)
 *       `can_publish(ctx)` — Editor and above. Below that the API silently
 *       downgrades the value to "internal", so the UI should not offer it.
 *
 * Saved queries are deliberately absent: `POST/PUT/DELETE /lab/saved-queries`
 * carry only `require_auth`, with no role floor, so every signed-in user may
 * write them and there is nothing here to gate.
 *
 * Keep this file and `api/middleware/permissions.py` in step. `hooks/useRole`
 * binds these to the session so pages ask a question, not do arithmetic.
 */

export type UserRole = "Viewer" | "Analyst" | "Editor" | "Admin";

export const ROLE_LEVELS: Record<UserRole, number> = {
	Viewer: 0,
	Analyst: 1,
	Editor: 2,
	Admin: 3,
};

/** True when `role` sits at or above `min`. An unknown role is below everything. */
export function roleAtLeast(role: UserRole | null | undefined, min: UserRole): boolean {
	if (!role || !(role in ROLE_LEVELS)) return false;
	return ROLE_LEVELS[role] >= ROLE_LEVELS[min];
}

/** `require_min_role("Analyst")` — may create charts, dashboards and datasets. */
export function canCreateContent(role: UserRole | null | undefined): boolean {
	return roleAtLeast(role, "Analyst");
}

/** `can_publish` — may set visibility to "published". */
export function canPublishContent(role: UserRole | null | undefined): boolean {
	return roleAtLeast(role, "Editor");
}

/**
 * `can_write` — may edit or delete the item owned by `ownerEmail`.
 *
 * Ownership is compared the way the API compares it: the session identity and
 * `created_by` both come from the same OAuth email, so an exact match on the
 * trimmed values is the same test the server runs. An item whose owner is
 * unknown is not writable by an Analyst, which is again what the API answers
 * when `created_by` is null.
 */
export function canWriteContent(
	role: UserRole | null | undefined,
	userEmail: string | null | undefined,
	ownerEmail: string | null | undefined,
): boolean {
	if (roleAtLeast(role, "Editor")) return true;
	if (!roleAtLeast(role, "Analyst")) return false;
	const self = (userEmail ?? "").trim();
	const owner = (ownerEmail ?? "").trim();
	return self !== "" && self === owner;
}

/**
 * Why an action is unavailable, for a `title`/`aria-label` on an affordance
 * that has to stay on screen. Professional register, no blame, says what the
 * reader would need instead.
 */
export function writeDeniedReason(
	role: UserRole | null | undefined,
	kind: string,
): string {
	if (!roleAtLeast(role, "Analyst")) {
		return `Your role (${role ?? "none"}) has read-only access. The Analyst role or higher is required to change a ${kind}.`;
	}
	return `This ${kind} belongs to another user. The Editor role or higher is required to change it.`;
}

/** Why creating is unavailable. */
export function createDeniedReason(
	role: UserRole | null | undefined,
	kind: string,
): string {
	return `Your role (${role ?? "none"}) has read-only access. The Analyst role or higher is required to create a ${kind}.`;
}
