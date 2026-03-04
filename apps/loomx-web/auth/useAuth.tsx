/**
 * Authentication Module - Azure AD Integration via MSAL
 *
 * This module provides authentication functionality for LoomX using
 * Microsoft Authentication Library (MSAL) for Azure AD/Entra ID integration.
 *
 * Key Features:
 * - Single sign-on via Azure AD popup flow
 * - Session persistence with 6-hour cache
 * - Automatic session restoration on page reload
 * - Context-based auth state management (React Context API)
 * - Backend connection synchronization
 *
 * Architecture:
 * - AuthProvider: React Context Provider that wraps the entire app
 * - useAuth: Hook to access auth state and actions (login, logout)
 * - MSAL: Handles Azure AD OAuth2/OIDC flows
 * - Backend sync: Calls /api/connect to establish backend session
 *
 * Session Flow:
 * 1. User visits site → AuthProvider checks localStorage cache
 * 2. If cached auth exists → restore MSAL session silently
 * 3. If no cache or expired → show login button
 * 4. User clicks login → MSAL popup → Azure AD login
 * 5. On success → call backend /api/connect → cache session
 * 6. Subsequent visits use cached session (6-hour TTL)
 */

import React, {
	createContext,
	useCallback,
	useContext,
	useEffect,
	useState,
	type ReactNode,
} from "react";

import { loginRequest, msalInstance } from "./msalConfig";

// Singleton promise to ensure MSAL is initialized only once
let msalInitPromise: Promise<void> | null = null;

/**
 * Ensure MSAL library is initialized before any auth operations.
 * This prevents race conditions during app startup.
 */
async function ensureMsalInitialized(): Promise<void> {
	if (!msalInitPromise) {
		msalInitPromise = msalInstance.initialize();
	}
	await msalInitPromise;
}

import { API_BASE } from "../config";

// LocalStorage key for caching authentication state
const AUTH_CACHE_KEY = "fabric-explorer-auth-cache";

// Cache time-to-live: 6 hours (matches typical session durations)
const CACHE_TTL_MS = 6 * 60 * 60 * 1000;

/**
 * Simplified user account information from Azure AD.
 * Contains only the essential fields needed for display and identification.
 */
interface SimpleAccount {
	name?: string;       // Display name (e.g., "John Doe")
	username?: string;   // UPN (e.g., "john.doe@company.com")
	email?: string;      // Email address (usually same as username)
}

/**
 * Authentication context value exposed to all components via useAuth hook.
 * This interface defines the shape of auth state and actions available throughout the app.
 */
interface AuthContextValue {
	isAuthenticated: boolean;       // True if user has valid session
	isConnecting: boolean;          // True during MSAL initialization or login
	error: string | null;           // Last auth error message (if any)
	login: () => Promise<void>;     // Trigger Azure AD login flow
	logout: () => Promise<void>;    // Clear session and disconnect
	account: SimpleAccount | null;  // Current user info (null if not authenticated)
}

// Create React Context for auth state (consumed via useAuth hook)
const AuthContext = createContext<AuthContextValue | undefined>(undefined);

/**
 * Check localStorage for a cached authentication session.
 *
 * Returns cached session info if:
 * - Cache exists and is valid JSON
 * - Timestamp is within TTL (6 hours)
 * - authenticated flag is true
 *
 * Otherwise, clears stale cache and returns null.
 *
 * This optimization allows instant page loads for returning users without
 * waiting for full MSAL token refresh on every visit.
 */
function getCachedAuth(): { timestamp: number; authenticated: boolean } | null {
	if (typeof window === "undefined") return null;  // SSR safety
	try {
		const raw = window.localStorage.getItem(AUTH_CACHE_KEY);
		if (!raw) return null;

		const parsed = JSON.parse(raw) as { timestamp?: number; authenticated?: boolean };
		if (!parsed || !parsed.timestamp || !parsed.authenticated) return null;

		const now = Date.now();
		if (now - parsed.timestamp > CACHE_TTL_MS) {
			// Cache expired, clean up
			window.localStorage.removeItem(AUTH_CACHE_KEY);
			return null;
		}

		return { timestamp: parsed.timestamp, authenticated: true };
	} catch {
		// Invalid cache data, ignore
		return null;
	}
}

/**
 * AuthProvider - React Context Provider for authentication state.
 *
 * This component wraps the entire application (see _app.tsx) and manages:
 * - MSAL initialization
 * - Session restoration from cache
 * - Login/logout actions
 * - User account information
 * - Backend connection synchronization
 *
 * On mount, it checks for cached auth and attempts to restore the session
 * silently. If successful, the user is immediately authenticated without
 * requiring a login action.
 *
 * All child components can access auth state via the useAuth hook.
 */
export function AuthProvider({ children }: { children: React.ReactNode }) {
	const [isAuthenticated, setIsAuthenticated] = useState(false);
	const [isConnecting, setIsConnecting] = useState(true);
	const [error, setError] = useState<string | null>(null);
	const [account, setAccount] = useState<SimpleAccount | null>(null);

	useEffect(() => {
		if (typeof window === "undefined") {
			setIsConnecting(false);
			return;
		}

		(async () => {
			try {
				// CRITICAL: Handle redirect first before any other MSAL operations
				await ensureMsalInitialized();
				const redirectResponse = await msalInstance.handleRedirectPromise();

				// If we just completed a redirect, set up the session
				if (redirectResponse) {
					const primaryAccount = redirectResponse.account ?? msalInstance.getAllAccounts()[0];
					if (primaryAccount) {
						try {
							// Fire /api/connect and theme prefetch in parallel.
							// Theme is written to localStorage before setIsAuthenticated so
							// ThemeContext's useState() initialiser reads it synchronously
							// on the very first render — no flash, no extra API round-trip.
							const [connectRes] = await Promise.all([
								fetch(`${API_BASE}/api/connect`, {
									method: "POST",
									headers: { "Content-Type": "application/json" },
									body: JSON.stringify({ initialize_only: true }),
								}),
								// Non-fatal: prefetch theme into localStorage
								msalInstance.acquireTokenSilent({ ...loginRequest, account: primaryAccount })
									.then(tok => fetch(`${API_BASE}/api/v1/theme`, {
										headers: {
											'Authorization': `Bearer ${tok.accessToken}`,
											'x-user-email': primaryAccount.username,
										},
									}))
									.then(r => r.ok ? r.json() : null)
									.then((themeData: any) => {
										if (themeData?.theme_color && typeof window !== 'undefined') {
											window.localStorage.setItem('loomx-theme-color', themeData.theme_color);
										}
									})
									.catch(() => { /* non-fatal */ }),
							]);
							const connectData = await connectRes.json();
							if (connectRes.ok && connectData?.success) {
								setIsAuthenticated(true);
								setAccount({
									name: primaryAccount.name ?? undefined,
									username: primaryAccount.username,
									email: primaryAccount.username,
								});
								window.localStorage.setItem(
									AUTH_CACHE_KEY,
									JSON.stringify({ authenticated: true, timestamp: Date.now() })
								);
								setIsConnecting(false);
								return;
							}
						} catch (err) {
							console.error("Backend connection failed:", err);
						}
					}
				}

				// Check cached auth if no redirect
				const cached = getCachedAuth();
				if (!cached) {
					setIsConnecting(false);
					return;
				}

				const accounts = msalInstance.getAllAccounts();
				const primary = accounts[0];
				if (!primary) {
					window.localStorage.removeItem(AUTH_CACHE_KEY);
					setIsConnecting(false);
					return;
				}

				const res = await fetch(`${API_BASE}/api/connect`, {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify({ use_cached: true }),
				});
				const data = await res.json();
				if (!res.ok || !data?.success) {
					window.localStorage.removeItem(AUTH_CACHE_KEY);
					setIsConnecting(false);
					return;
				}

				setIsAuthenticated(true);
				setAccount({
					name: primary.name ?? undefined,
					username: primary.username,
					email: primary.username,
				});
			} catch (err) {
				console.error("Auth initialization error:", err);
				window.localStorage.removeItem(AUTH_CACHE_KEY);
			} finally {
				setIsConnecting(false);
			}
		})();
	}, []);

	const login = useCallback(async () => {
		setError(null);
		setIsConnecting(true);
		try {
			if (typeof window === "undefined") {
				throw new Error("Login is only available in the browser");
			}

			await ensureMsalInitialized();

			// Use redirect flow instead of popup (better for enterprise environments)
			await msalInstance.loginRedirect(loginRequest);
			// After redirect, the useEffect will handle the callback and set up the session
		} catch (e) {
			const message = e instanceof Error ? e.message : "Connection failed";
			setError(message);
			setIsConnecting(false);
		}
	}, []);

	const logout = useCallback(async () => {
		setError(null);
		try {
			await fetch(`${API_BASE}/api/disconnect`, {
				method: "POST",
				headers: { "Content-Type": "application/json" },
			});
		} catch {
			// Ignore disconnect errors; we still clear local state
		} finally {
			if (typeof window !== "undefined") {
				window.localStorage.removeItem(AUTH_CACHE_KEY);
				// Clear theme preference so next login shows default Microsoft blue
				window.localStorage.removeItem('loomx-theme-color');
			}
			setIsAuthenticated(false);
			setAccount(null);
		}
	}, []);

	const cachedName = account?.name;

	const value: AuthContextValue = {
		isAuthenticated,
		isConnecting,
		error,
		login,
		logout,
		account: isAuthenticated ? account : null,
	};

	return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

/**
 * useAuth - React Hook to access authentication state and actions.
 *
 * This hook provides access to the authentication context created by AuthProvider.
 * It must be called from within a component that's wrapped by AuthProvider
 * (which is all components in this app, since AuthProvider wraps _app.tsx).
 *
 * Usage:
 * ```tsx
 * const { isAuthenticated, login, logout, account } = useAuth();
 *
 * if (!isAuthenticated) {
 *   return <button onClick={login}>Sign in with Azure AD</button>;
 * }
 *
 * return <div>Welcome, {account?.name}</div>;
 * ```
 *
 * @throws Error if used outside of AuthProvider
 * @returns AuthContextValue with auth state and actions
 */
export function useAuth(): AuthContextValue {
	const ctx = useContext(AuthContext);
	if (!ctx) {
		throw new Error("useAuth must be used within an AuthProvider");
	}
	return ctx;
}