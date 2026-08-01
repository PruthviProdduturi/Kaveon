import { loginRequest, getMsalInstance } from "../auth/msalConfig";

import { API_BASE } from "../config";

/**
 * Acquire a valid access token for API calls using MSAL.
 * Returns a Promise that resolves to the access token string.
 * Throws if no account is found or token acquisition fails.
 */
export async function getAccessToken(): Promise<string> {
  const accounts = getMsalInstance().getAllAccounts();
  if (!accounts.length) throw new Error("No account found for authentication");
  try {
    const tokenResponse = await getMsalInstance().acquireTokenSilent({ ...loginRequest, account: accounts[0] });
    return tokenResponse.accessToken;
  } catch (silentErr) {
    // Silent token acquisition failed (session expired or interaction required).
    // Try popup interactive flow so UI actions don't redirect the page.
    try {
      const popupResult = await getMsalInstance().acquireTokenPopup(loginRequest);
      return popupResult.accessToken;
    } catch (popupErr) {
      // Re-throw the original silent error for callers to handle.
      throw silentErr;
    }
  }
}

/**
 * Helper for making authenticated API calls.
 * For local auth (lens_auth_provider === "local"), reads the Bearer token from
 * localStorage instead of going through MSAL.
 * For azure_ad (default), uses the existing MSAL token acquisition flow.
 * Usage: await msalFetch(url, { method: "GET" })
 * Automatically adds Authorization and x-user-email headers.
 */
export async function msalFetch(input: RequestInfo, init: RequestInit = {}): Promise<Response> {
  const headers = new Headers(init.headers || {});

  const storedProvider = typeof window !== "undefined" ? window.localStorage.getItem("lens_auth_provider") : null;
  if (!storedProvider || storedProvider === "local") {
    // Local auth: use stored JWT directly, no MSAL involved
    const localToken = window.localStorage.getItem("lens_local_token");
    if (localToken) {
      headers.set("Authorization", `Bearer ${localToken}`);
    }
  } else {
    // Azure AD auth: acquire token via MSAL (may not be configured yet if API is still starting)
    try {
      const tokenStart = performance.now();
      const token = await getAccessToken();
      const tokenEnd = performance.now();
      if (tokenEnd - tokenStart > 100) {
        console.log(`[msalFetch] Token acquisition took ${(tokenEnd - tokenStart).toFixed(2)}ms`);
      }

      const accounts = getMsalInstance().getAllAccounts();
      const userEmail = accounts[0]?.username || accounts[0]?.name || null;

      headers.set("Authorization", `Bearer ${token}`);
      if (userEmail) headers.set("x-user-email", userEmail);
    } catch {
      // MSAL not yet configured (API still starting) — request proceeds without auth header,
      // will return 401 and the auth layer will handle it.
    }
  }

  let fetchInput: RequestInfo = input;
  if (typeof input === "string" && input.startsWith("/api")) {
    // Always use API_BASE for all /api calls, regardless of environment
    const base = API_BASE.endsWith("/") ? API_BASE.slice(0, -1) : API_BASE;
    fetchInput = `${base}${input}`;
  }
  return fetch(fetchInput, { ...init, headers });
}
