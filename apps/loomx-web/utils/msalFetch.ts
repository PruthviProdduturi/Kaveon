import { InteractionRequiredAuthError } from "@azure/msal-browser";
import { loginRequest, msalInstance } from "../auth/msalConfig";

import { API_BASE } from "../config";

// Fabric SQL delegated permission scope.
// Requesting this scope lets the proxy authenticate to Fabric SQL as the
// signed-in user — no service principal or `az login` required.
const FABRIC_SQL_SCOPE = "https://database.windows.net/user_impersonation";

/**
 * Acquire a valid access token for API calls using MSAL.
 * Returns a Promise that resolves to the access token string.
 * Throws if no account is found or token acquisition fails.
 */
export async function getAccessToken(): Promise<string> {
  const accounts = msalInstance.getAllAccounts();
  if (!accounts.length) throw new Error("No account found for authentication");
  try {
    const tokenResponse = await msalInstance.acquireTokenSilent({ ...loginRequest, account: accounts[0] });
    return tokenResponse.accessToken;
  } catch (silentErr) {
    // Silent token acquisition failed (session expired or interaction required).
    // Try popup interactive flow so UI actions don't redirect the page.
    try {
      const popupResult = await msalInstance.acquireTokenPopup(loginRequest);
      return popupResult.accessToken;
    } catch (popupErr) {
      // Re-throw the original silent error for callers to handle.
      throw silentErr;
    }
  }
}

/**
 * Acquire a Fabric SQL delegated token for the signed-in user.
 * This token is forwarded to the Python proxy so it connects to Fabric SQL
 * under the user's own Azure AD identity instead of a service account.
 *
 * Returns null (non-fatal) if the scope hasn't been consented yet or if the
 * user has no account — the proxy will fall back to DefaultAzureCredential.
 * On first-ever use an interactive popup handles the one-time consent.
 */
async function getFabricToken(): Promise<string | null> {
  try {
    const accounts = msalInstance.getAllAccounts();
    if (!accounts.length) return null;
    const tokenResponse = await msalInstance.acquireTokenSilent({
      account: accounts[0],
      scopes: [FABRIC_SQL_SCOPE],
    });
    return tokenResponse.accessToken;
  } catch (err) {
    if (err instanceof InteractionRequiredAuthError) {
      // First time: scope needs interactive consent — show popup once.
      try {
        const accounts = msalInstance.getAllAccounts();
        const popupResult = await msalInstance.acquireTokenPopup({
          account: accounts[0],
          scopes: [FABRIC_SQL_SCOPE],
        });
        return popupResult.accessToken;
      } catch {
        return null; // consent declined or blocked — proxy falls back to service identity
      }
    }
    return null; // any other error — degrade gracefully
  }
}

/**
 * Helper for making authenticated API calls with MSAL token.
 * Usage: await msalFetch(url, { method: "GET" })
 * Automatically adds Authorization, x-user-email, and x-fabric-token headers.
 * x-fabric-token is the user's delegated Fabric SQL token; the Python proxy
 * uses it to run SQL under the user's identity instead of a service account.
 */
export async function msalFetch(input: RequestInfo, init: RequestInit = {}): Promise<Response> {
  const tokenStart = performance.now();
  // Acquire both tokens in parallel — Fabric token is a second silent MSAL
  // call (fast, cached) and does not block the API token acquisition.
  const [token, fabricToken] = await Promise.all([
    getAccessToken(),
    getFabricToken(),
  ]);
  const tokenEnd = performance.now();
  if (tokenEnd - tokenStart > 100) {
    console.log(`[msalFetch] Token acquisition took ${(tokenEnd - tokenStart).toFixed(2)}ms`);
  }

  const accounts = msalInstance.getAllAccounts();
  const userEmail = accounts[0]?.username || accounts[0]?.name || null;

  const headers = new Headers(init.headers || {});
  headers.set("Authorization", `Bearer ${token}`);

  // Add user email header for backend to identify the user
  if (userEmail) {
    headers.set("x-user-email", userEmail);
  }

  // Forward Fabric SQL delegated token so the proxy can authenticate as
  // this user rather than falling back to a service account.
  if (fabricToken) {
    headers.set("x-fabric-token", fabricToken);
  }

  let fetchInput: RequestInfo = input;
  if (typeof input === "string" && input.startsWith("/api")) {
    // Always use API_BASE for all /api calls, regardless of environment
    const base = API_BASE.endsWith("/") ? API_BASE.slice(0, -1) : API_BASE;
    fetchInput = `${base}${input}`;
  }
  return fetch(fetchInput, { ...init, headers });
}
