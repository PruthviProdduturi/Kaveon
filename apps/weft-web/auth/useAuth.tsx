/**
 * Authentication — Kaveon Identity (suite-wide SSO).
 *
 * Weft delegates sign-in to the Kaveon Identity gateway. The user authenticates
 * once (Microsoft work/school/personal, or Google) and the gateway issues a
 * shared session honoured across the whole suite. This module just reads that
 * session (GET {IDENTITY_BASE}/api/auth/me) and hands off to the gateway for
 * sign-in/out. No local passwords, no dev logins.
 */

"use client";

import React, {
	createContext,
	useCallback,
	useContext,
	useEffect,
	useState,
} from "react";

import { IDENTITY_BASE } from "../config";

export type AuthProvider = "microsoft" | "google";
export type UserRole = "Viewer" | "Analyst" | "Editor" | "Admin";

interface SimpleAccount {
	name?: string;
	username?: string;
	email?: string;
}

interface AuthContextValue {
	isAuthenticated: boolean;
	isConnecting: boolean;
	noAccess: boolean;
	error: string | null;
	/** Start sign-in via the gateway. `provider` is "microsoft" (default) or "google". */
	login: (provider?: string) => Promise<void>;
	logout: () => Promise<void>;
	account: SimpleAccount | null;
	role: UserRole | null;
	provider: AuthProvider | null;
}

const AuthContext = createContext<AuthContextValue | undefined>(undefined);

export function AuthProvider({ children }: { children: React.ReactNode }) {
	const [isAuthenticated, setIsAuthenticated] = useState(false);
	const [isConnecting, setIsConnecting] = useState(true);
	const [noAccess, setNoAccess] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const [account, setAccount] = useState<SimpleAccount | null>(null);
	const [role, setRole] = useState<UserRole | null>(null);
	const [provider, setProvider] = useState<AuthProvider | null>(null);

	// Restore the suite session from the gateway on mount.
	useEffect(() => {
		if (typeof window === "undefined") {
			setIsConnecting(false);
			return;
		}
		(async () => {
			try {
				const res = await fetch(`${IDENTITY_BASE}/api/auth/me`, {
					credentials: "include",
				});
				if (res.ok) {
					const data = (await res.json()) as {
						email: string;
						name: string;
						role: UserRole;
						provider: AuthProvider;
					};
					setIsAuthenticated(true);
					setAccount({ name: data.name, username: data.email, email: data.email });
					setRole(data.role);
					setProvider(data.provider);
				}
			} catch {
				// Gateway unreachable / not signed in → show the sign-in screen.
			} finally {
				setIsConnecting(false);
			}
		})();
	}, []);

	const login = useCallback(async (prov: string = "microsoft"): Promise<void> => {
		setError(null);
		const next = encodeURIComponent(window.location.origin + "/");
		window.location.href =
			`${IDENTITY_BASE}/oauth2/sign_in?provider=${prov}&next=${next}`;
	}, []);

	const logout = useCallback(async (): Promise<void> => {
		setIsAuthenticated(false);
		setAccount(null);
		setRole(null);
		setNoAccess(false);
		window.location.href = `${IDENTITY_BASE}/oauth2/sign_out`;
	}, []);

	const value: AuthContextValue = {
		isAuthenticated,
		isConnecting,
		noAccess,
		error,
		login,
		logout,
		account: isAuthenticated ? account : null,
		role: isAuthenticated ? role : null,
		provider,
	};

	return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

/**
 * useAuth — access authentication state and actions.
 * @throws if used outside of AuthProvider
 */
export function useAuth(): AuthContextValue {
	const ctx = useContext(AuthContext);
	if (!ctx) {
		throw new Error("useAuth must be used within an AuthProvider");
	}
	return ctx;
}
