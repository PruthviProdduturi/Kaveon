import { useAuth, type UserRole } from "../auth/useAuth";

const ROLE_LEVELS: Record<UserRole, number> = {
	Viewer: 0,
	Analyst: 1,
	Editor: 2,
	Admin: 3,
};

function atLeast(role: UserRole | null, min: UserRole): boolean {
	if (!role) return false;
	return ROLE_LEVELS[role] >= ROLE_LEVELS[min];
}

export function useRole() {
	const { role, isConnecting } = useAuth();

	return {
		role,
		loading: isConnecting,
		isViewer: role !== null,                    // all authenticated users are at least Viewer
		isAnalyst: atLeast(role, "Analyst"),
		isEditor: atLeast(role, "Editor"),
		isAdmin: atLeast(role, "Admin"),
		canCreate: atLeast(role, "Analyst"),         // create datasets/charts/dashboards
		canPublish: atLeast(role, "Editor"),         // set visibility to "published"
		canManageUsers: atLeast(role, "Admin"),
	};
}
