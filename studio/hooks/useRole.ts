import { useCallback, useMemo } from "react";

import { useAuth } from "../auth/useAuth";
import {
	canCreateContent,
	canPublishContent,
	canWriteContent,
	createDeniedReason,
	roleAtLeast,
	writeDeniedReason,
	type UserRole,
} from "../utils/permissions";

/**
 * The signed-in user's role, and what that role may do.
 *
 * The predicates come from `utils/permissions`, which mirrors
 * `api/middleware/permissions.py` — so a page asks this hook rather than
 * reimplementing the rule, and there is one place to change when the API's
 * rule changes.
 */
export function useRole() {
	const { role, isConnecting, account } = useAuth();
	const email = account?.email ?? account?.username ?? null;

	/**
	 * May this user edit or delete the item owned by `ownerEmail`? Bound to the
	 * session, so a caller passes only the item's `created_by`.
	 */
	const canWrite = useCallback(
		(ownerEmail: string | null | undefined) => canWriteContent(role, email, ownerEmail),
		[role, email],
	);

	/** The reason an edit or delete is unavailable, for a title/aria-label. */
	const whyNoWrite = useCallback(
		(kind: string) => writeDeniedReason(role, kind),
		[role],
	);

	/** The reason a create is unavailable, for a title/aria-label. */
	const whyNoCreate = useCallback(
		(kind: string) => createDeniedReason(role, kind),
		[role],
	);

	return useMemo(() => ({
		role: role as UserRole | null,
		email,
		loading: isConnecting,
		isViewer: role !== null,                     // all authenticated users are at least Viewer
		isAnalyst: roleAtLeast(role, "Analyst"),
		isEditor: roleAtLeast(role, "Editor"),
		isAdmin: roleAtLeast(role, "Admin"),
		canCreate: canCreateContent(role),           // create datasets/charts/dashboards
		canPublish: canPublishContent(role),         // set visibility to "published"
		canManageUsers: roleAtLeast(role, "Admin"),
		canWrite,
		whyNoWrite,
		whyNoCreate,
	}), [role, email, isConnecting, canWrite, whyNoWrite, whyNoCreate]);
}
