/**
 * Authentication — NextAuth session (OAuth only: GitHub, Google, Microsoft).
 *
 * Lens is self-contained: sign-in runs inside the Next.js app via NextAuth
 * (see ../auth.ts). This hook exposes the session to the rest of the app in the
 * same shape components already expect. No local passwords, no external gateway.
 */

"use client";

import React, { createContext, useCallback, useContext } from "react";
import { SessionProvider as NextAuthSessionProvider, useSession, signIn, signOut } from "next-auth/react";

export type AuthProvider = "github" | "google" | "microsoft-entra-id";
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
	/** Start sign-in with a provider ("github" | "google" | "microsoft-entra-id"). */
	login: (provider?: AuthProvider) => Promise<void>;
	logout: () => Promise<void>;
	account: SimpleAccount | null;
	role: UserRole | null;
	provider: AuthProvider | null;
}

const AuthContext = createContext<AuthContextValue | undefined>(undefined);

/**
 * AuthProvider — derives auth state from the NextAuth session.
 * Must be rendered inside next-auth's <SessionProvider> (see app/providers.tsx).
 */
export function AuthProvider({ children }: { children: React.ReactNode }) {
	const { data: session, status } = useSession();

	const login = useCallback(async (provider?: AuthProvider) => {
		await signIn(provider, { callbackUrl: "/" });
	}, []);

	const logout = useCallback(async () => {
		await signOut({ callbackUrl: "/login" });
	}, []);

	const user = session?.user ?? null;
	const value: AuthContextValue = {
		isAuthenticated: status === "authenticated",
		isConnecting: status === "loading",
		noAccess: false,
		error: null,
		login,
		logout,
		account: user
			? { name: user.name ?? undefined, username: user.email ?? undefined, email: user.email ?? undefined }
			: null,
		role: (user as { role?: UserRole } | null)?.role ?? (user ? "Viewer" : null),
		provider: null,
	};

	return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

/** Re-export NextAuth's SessionProvider for the app root. */
export { NextAuthSessionProvider };

export function useAuth(): AuthContextValue {
	const ctx = useContext(AuthContext);
	if (!ctx) {
		throw new Error("useAuth must be used within an AuthProvider");
	}
	return ctx;
}
