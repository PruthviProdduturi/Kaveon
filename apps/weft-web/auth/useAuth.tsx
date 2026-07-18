/**
 * Authentication Module - Multi-Provider (Local, Azure AD, Google)
 *
 * This module provides authentication functionality for Weft supporting
 * multiple auth providers: local username/password, Azure AD via MSAL, and
 * Google OAuth2 (stub).
 *
 * On mount, the provider fetches GET /api/auth/provider to determine the
 * active auth method, then initialises the appropriate flow.
 *
 * Token storage keys for local auth:
 *   weft_local_token     — JWT string
 *   weft_local_token_exp — exp timestamp as number
 *   weft_auth_provider   — "local" | "azure_ad" | "google"
 */

import React, {
	createContext,
	useCallback,
	useContext,
	useEffect,
	useState,
	type ReactNode,
} from "react";

import { loginRequest, configureMsal, getMsalInstance, ensureMsalInitialized } from "./msalConfig";

import { API_BASE } from "../config";

// LocalStorage key for caching authentication state (Azure AD)
const AUTH_CACHE_KEY = "weft-auth-cache";
// LocalStorage key for caching resolved role
const ROLE_CACHE_KEY = "weft-user-role";
// LocalStorage key for the active auth provider
const PROVIDER_KEY = "weft_auth_provider";
// LocalStorage key for local auth JWT
const LOCAL_TOKEN_KEY = "weft_local_token";
// LocalStorage key for local auth token expiry
const LOCAL_TOKEN_EXP_KEY = "weft_local_token_exp";

// Cache time-to-live: 6 hours (matches typical session durations)
const CACHE_TTL_MS = 6 * 60 * 60 * 1000;

export type AuthProvider = "local" | "azure_ad" | "google";

/**
 * Simplified user account information.
 * Contains only the essential fields needed for display and identification.
 */
interface SimpleAccount {
	name?: string;       // Display name (e.g., "John Doe")
	username?: string;   // UPN / username
	email?: string;      // Email address
}

/**
 * Authentication context value exposed to all components via useAuth hook.
 */
export type UserRole = "Viewer" | "Analyst" | "Editor" | "Admin";

interface AuthContextValue {
	isAuthenticated: boolean;
	isConnecting: boolean;
	noAccess: boolean;
	error: string | null;
	login: (credentials?: { username: string; password: string }) => Promise<void>;
	logout: () => Promise<void>;
	account: SimpleAccount | null;
	role: UserRole | null;
	provider: AuthProvider | null;
}

// Create React Context for auth state (consumed via useAuth hook)
const AuthContext = createContext<AuthContextValue | undefined>(undefined);

/**
 * Parse a JWT payload without verifying the signature.
 * Safe to use on the frontend for reading claims only.
 */
function parseJwtPayload(token: string): Record<string, unknown> {
	try {
		return JSON.parse(atob(token.split(".")[1]));
	} catch {
		return {};
	}
}

const _ROLE_LEVELS: Record<string, number> = { Viewer: 0, Analyst: 1, Editor: 2, Admin: 3 };

/**
 * Extract the highest-priority role from a JWT payload.
 * The backend issues `roles: string[]`; some providers use `role: string`.
 */
function roleFromPayload(payload: Record<string, unknown>): UserRole {
	const arr = Array.isArray(payload.roles)
		? (payload.roles as string[]).filter((r) => r in _ROLE_LEVELS)
		: [];
	if (arr.length > 0) {
		return arr.reduce((best, r) =>
			(_ROLE_LEVELS[r] ?? 0) > (_ROLE_LEVELS[best] ?? 0) ? r : best
		) as UserRole;
	}
	// Fallback: singular `role` claim (some providers)
	const singular = payload.role as string | undefined;
	if (singular && singular in _ROLE_LEVELS) return singular as UserRole;
	return "Viewer";
}

/**
 * Check localStorage for a cached Azure AD authentication session.
 */
function getCachedAuth(): { timestamp: number; authenticated: boolean } | null {
	if (typeof window === "undefined") return null;
	try {
		const raw = window.localStorage.getItem(AUTH_CACHE_KEY);
		if (!raw) return null;

		const parsed = JSON.parse(raw) as { timestamp?: number; authenticated?: boolean };
		if (!parsed || !parsed.timestamp || !parsed.authenticated) return null;

		const now = Date.now();
		if (now - parsed.timestamp > CACHE_TTL_MS) {
			window.localStorage.removeItem(AUTH_CACHE_KEY);
			return null;
		}

		return { timestamp: parsed.timestamp, authenticated: true };
	} catch {
		return null;
	}
}

/**
 * AuthProvider - React Context Provider for authentication state.
 *
 * On mount it calls GET /api/auth/provider, stores the result, then runs the
 * appropriate initialisation flow for the active provider.
 */
export function AuthProvider({ children }: { children: React.ReactNode }) {
	const [isAuthenticated, setIsAuthenticated] = useState(false);
	const [isConnecting, setIsConnecting] = useState(true);
	const [noAccess, setNoAccess] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const [account, setAccount] = useState<SimpleAccount | null>(null);
	const [role, setRole] = useState<UserRole | null>(null);
	const [provider, setProvider] = useState<AuthProvider | null>(null);

	useEffect(() => {
		if (typeof window === "undefined") {
			setIsConnecting(false);
			return;
		}

		(async () => {
			try {
				// ── 1. Resolve the active auth provider ──────────────────────────────
				let resolvedProvider: AuthProvider = "local";
				try {
					const _providerAbort = new AbortController();
					const _providerTimeout = setTimeout(() => _providerAbort.abort(), 4000);
					const providerRes = await fetch(`${API_BASE}/api/auth/provider`, { signal: _providerAbort.signal }).finally(() => clearTimeout(_providerTimeout));
					if (providerRes.ok) {
						const providerData = await providerRes.json() as {
							provider: AuthProvider;
							azure_client_id?: string;
							azure_tenant_id?: string;
							google_client_id?: string;
						};
						resolvedProvider = providerData.provider;
						// Configure MSAL with the runtime client/tenant IDs from the API
						if (providerData.provider === "azure_ad" && providerData.azure_client_id) {
							configureMsal(
								providerData.azure_client_id,
								providerData.azure_tenant_id ?? "common",
							);
						}
					}
				} catch {
					// If the provider endpoint fails, fall back to whatever is cached
					const cached = typeof window !== "undefined"
						? window.localStorage.getItem(PROVIDER_KEY) as AuthProvider | null
						: null;
					if (cached) resolvedProvider = cached;
				}
				window.localStorage.setItem(PROVIDER_KEY, resolvedProvider);
				setProvider(resolvedProvider);

				// ── 2. Provider-specific session restoration ─────────────────────────

				if (resolvedProvider === "local") {
					// Local auth: check stored JWT validity
					const token = window.localStorage.getItem(LOCAL_TOKEN_KEY);
					const expRaw = window.localStorage.getItem(LOCAL_TOKEN_EXP_KEY);
					const exp = expRaw ? Number(expRaw) : 0;

					if (token && exp && Date.now() / 1000 < exp) {
						const payload = parseJwtPayload(token);
						const cachedRole = window.localStorage.getItem(ROLE_CACHE_KEY) as UserRole | null;
						setIsAuthenticated(true);
						setAccount({
							name: (payload.name as string) ?? (payload.sub as string) ?? undefined,
							username: (payload.sub as string) ?? undefined,
							email: (payload.email as string) ?? (payload.sub as string) ?? undefined,
						});
						setRole(roleFromPayload(payload) ?? cachedRole);
					}
					// If token is missing or expired, stay unauthenticated → show login screen
					setIsConnecting(false);
					return;
				}

				if (resolvedProvider === "google") {
					// Google stub — not yet implemented
					setIsConnecting(false);
					return;
				}

				// ── Azure AD flow (unchanged) ─────────────────────────────────────────
				try {
					await ensureMsalInitialized();
				} catch {
					setIsConnecting(false);
					return;
				}
				const redirectResponse = await getMsalInstance().handleRedirectPromise();

				if (redirectResponse) {
					const primaryAccount = redirectResponse.account ?? getMsalInstance().getAllAccounts()[0];
					if (primaryAccount) {
						try {
							const tok = await getMsalInstance().acquireTokenSilent({ ...loginRequest, account: primaryAccount });
							const authHeaders = {
								"Authorization": `Bearer ${tok.accessToken}`,
								"x-user-email": primaryAccount.username,
							};

							// Theme is non-blocking — fire and forget so it doesn't delay auth state
						fetch(`${API_BASE}/api/v1/theme`, { headers: authHeaders })
							.then(r => r.ok ? r.json() : null)
							.then((themeData: any) => {
								if (themeData?.theme_color && typeof window !== "undefined") {
									window.localStorage.setItem("weft-theme-color", themeData.theme_color);
								}
							})
							.catch(() => { /* non-fatal */ });

						const [connectRes] = await Promise.all([
							fetch(`${API_BASE}/api/connect`, {
								method: "POST",
								headers: { "Content-Type": "application/json" },
								body: JSON.stringify({ initialize_only: true }),
							}),
							fetch(`${API_BASE}/api/v1/users/me`, { headers: authHeaders })
								.then(r => r.ok ? r.json() : null)
								.then((meData: any) => {
									if (meData?.role && typeof window !== "undefined") {
										window.localStorage.setItem(ROLE_CACHE_KEY, meData.role);
									}
								})
								.catch(() => { /* non-fatal */ }),
						]);
							const connectData = await connectRes.json();
							if (connectRes.ok && connectData?.success) {
								const cachedRole = window.localStorage.getItem(ROLE_CACHE_KEY) as UserRole | null;
								setIsAuthenticated(true);
								setAccount({
									name: primaryAccount.name ?? undefined,
									username: primaryAccount.username,
									email: primaryAccount.username,
								});
								setRole(cachedRole);
								window.localStorage.setItem(
									AUTH_CACHE_KEY,
									JSON.stringify({ authenticated: true, timestamp: Date.now() })
								);
								await checkAccess({
									"Authorization": `Bearer ${tok.accessToken}`,
									"x-user-email": primaryAccount.username,
								});
								setIsConnecting(false);
								return;
							}
						} catch (err) {
							console.error("Backend connection failed:", err);
						}
					}
				}

				const cached = getCachedAuth();
				if (!cached) {
					setIsConnecting(false);
					return;
				}

				const accounts = getMsalInstance().getAllAccounts();
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

				const cachedRole = window.localStorage.getItem(ROLE_CACHE_KEY) as UserRole | null;
				setIsAuthenticated(true);
				setAccount({
					name: primary.name ?? undefined,
					username: primary.username,
					email: primary.username,
				});
				setRole(cachedRole);

				// Re-fetch role in background
				getMsalInstance().acquireTokenSilent({ ...loginRequest, account: primary })
					.then(tok => fetch(`${API_BASE}/api/v1/users/me`, {
						headers: {
							"Authorization": `Bearer ${tok.accessToken}`,
							"x-user-email": primary.username,
						},
					}))
					.then(r => r.ok ? r.json() : null)
					.then((meData: any) => {
						if (meData?.role) {
							window.localStorage.setItem(ROLE_CACHE_KEY, meData.role);
							setRole(meData.role as UserRole);
						}
					})
					.catch(() => { /* non-fatal */ });
			} catch (err) {
				console.error("Auth initialization error:", err);
				window.localStorage.removeItem(AUTH_CACHE_KEY);
			} finally {
				setIsConnecting(false);
			}
		})();
	}, []);

	const login = useCallback(async (credentials?: { username: string; password: string }) => {
		setError(null);
		setIsConnecting(true);
		try {
			if (typeof window === "undefined") {
				throw new Error("Login is only available in the browser");
			}

			const activeProvider = window.localStorage.getItem(PROVIDER_KEY) as AuthProvider | null;

			if (activeProvider === "local") {
				// Local username/password login
				if (!credentials) throw new Error("Username and password are required");
				const res = await fetch(`${API_BASE}/api/auth/login`, {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify(credentials),
				});
				if (!res.ok) {
					const body = await res.json().catch(() => ({}));
					throw new Error((body as any)?.detail ?? "Invalid username or password");
				}
				const data = await res.json() as { access_token: string; token_type: string; force_password_change?: boolean };
				const token = data.access_token;
				const payload = parseJwtPayload(token);
				const exp = (payload.exp as number) ?? 0;

				window.localStorage.setItem(LOCAL_TOKEN_KEY, token);
				window.localStorage.setItem(LOCAL_TOKEN_EXP_KEY, String(exp));

				const userRole = roleFromPayload(payload);
				window.localStorage.setItem(ROLE_CACHE_KEY, userRole);

				setIsAuthenticated(true);
				setAccount({
					name: (payload.name as string) ?? (payload.sub as string) ?? undefined,
					username: (payload.sub as string) ?? undefined,
					email: (payload.email as string) ?? (payload.sub as string) ?? undefined,
				});
				setRole(userRole);
				await checkAccess({ "Authorization": `Bearer ${token}` });
				return;
			}

			if (activeProvider === "google") {
				// Google stub
				alert("Google auth coming soon");
				return;
			}

			// Azure AD: redirect flow.
			// If MSAL wasn't configured on page load (API was still starting), try now.
			try { await ensureMsalInitialized(); } catch {
				const provRes = await fetch(`${API_BASE}/api/auth/provider`);
				if (provRes.ok) {
					const provData = await provRes.json() as { provider: AuthProvider; azure_client_id?: string; azure_tenant_id?: string };
					if (provData.azure_client_id) configureMsal(provData.azure_client_id, provData.azure_tenant_id ?? "common");
				}
				await ensureMsalInitialized();
			}
			await getMsalInstance().loginRedirect(loginRequest);
		} catch (e) {
			const message = e instanceof Error ? e.message : "Connection failed";
			setError(message);
		} finally {
			setIsConnecting(false);
		}
	}, []);

	const logout = useCallback(async () => {
		setError(null);
		const activeProvider = typeof window !== "undefined"
			? window.localStorage.getItem(PROVIDER_KEY) as AuthProvider | null
			: null;

		try {
			if (activeProvider !== "local") {
				await fetch(`${API_BASE}/api/disconnect`, {
					method: "POST",
					headers: { "Content-Type": "application/json" },
				});
			}
		} catch {
			// Ignore disconnect errors; we still clear local state
		} finally {
			if (typeof window !== "undefined") {
				window.localStorage.removeItem(AUTH_CACHE_KEY);
				window.localStorage.removeItem(LOCAL_TOKEN_KEY);
				window.localStorage.removeItem(LOCAL_TOKEN_EXP_KEY);
				window.localStorage.removeItem("weft-theme-color");
				window.localStorage.removeItem(ROLE_CACHE_KEY);
			}
			setIsAuthenticated(false);
			setAccount(null);
			setRole(null);
		}
	}, []);

	// After any login, verify the user has a role assigned
	const checkAccess = async (headers: Record<string, string>) => {
		try {
			const res = await fetch(`${API_BASE}/api/v1/users/me`, { headers });
			if (res.status === 403) {
				const body = await res.json().catch(() => ({}));
				if ((body as any)?.detail?.code === "no_role") {
					setNoAccess(true);
					setIsAuthenticated(false);
					return false;
				}
			}
			if (res.ok) {
				const data = await res.json().catch(() => ({}));
				if ((data as any)?.role) {
					setRole((data as any).role as UserRole);
					if (typeof window !== "undefined")
						window.localStorage.setItem(ROLE_CACHE_KEY, (data as any).role);
				}
			}
		} catch { /* non-fatal */ }
		return true;
	};

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
 * useAuth - React Hook to access authentication state and actions.
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
