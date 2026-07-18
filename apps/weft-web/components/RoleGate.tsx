import type { ReactNode } from "react";
import { useRole } from "../hooks/useRole";
import type { UserRole } from "../auth/useAuth";

const ROLE_LEVELS: Record<UserRole, number> = {
	Viewer: 0,
	Analyst: 1,
	Editor: 2,
	Admin: 3,
};

interface RoleGateProps {
	/** Minimum role required to render children */
	minRole: UserRole;
	children: ReactNode;
	/** Optional fallback rendered when role requirement is not met */
	fallback?: ReactNode;
}

/**
 * Renders children only when the authenticated user meets the minimum role.
 *
 * Usage:
 * ```tsx
 * <RoleGate minRole="Analyst">
 *   <CreateDatasetButton />
 * </RoleGate>
 *
 * <RoleGate minRole="Admin" fallback={<p>Admin access required.</p>}>
 *   <UserManagementPanel />
 * </RoleGate>
 * ```
 */
export function RoleGate({ minRole, children, fallback = null }: RoleGateProps) {
	const { role } = useRole();
	if (!role || ROLE_LEVELS[role] < ROLE_LEVELS[minRole]) {
		return <>{fallback}</>;
	}
	return <>{children}</>;
}
