import { Configuration, PublicClientApplication } from "@azure/msal-browser";

const apiScope = process.env.NEXT_PUBLIC_AZURE_API_SCOPE;

export const loginRequest = {
  scopes: apiScope ? [apiScope] : ["User.Read"],
};

// ── Lazy MSAL instance ────────────────────────────────────────────────────────
// Created by configureMsal() once the clientId/tenantId are known at runtime.
// Falls back to build-time env vars if NEXT_PUBLIC_AZURE_CLIENT_ID is set
// (supports direct env-var configuration in Docker / k8s deployments).

let _instance: PublicClientApplication | null = null;
let _initPromise: Promise<void> | null = null;

export function configureMsal(clientId: string, tenantId = "common"): void {
  const config: Configuration = {
    auth: {
      clientId,
      authority: `https://login.microsoftonline.com/${tenantId}`,
      redirectUri:
        process.env.NEXT_PUBLIC_AZURE_REDIRECT_URI ||
        (typeof window !== "undefined" ? window.location.origin : ""),
    },
    cache: { cacheLocation: "sessionStorage" },
    system: { allowRedirectInIframe: false },
  };
  _instance = new PublicClientApplication(config);
  _initPromise = null; // reset so ensureMsalInitialized re-initialises the new instance
}

export function getMsalInstance(): PublicClientApplication {
  if (!_instance) throw new Error("[MSAL] Not configured — call configureMsal() first.");
  return _instance;
}

export async function ensureMsalInitialized(): Promise<void> {
  if (!_instance) throw new Error("[MSAL] Not configured — call configureMsal() first.");
  if (!_initPromise) {
    _initPromise = _instance.initialize();
  }
  await _initPromise;
}

// Initialise from build-time env vars when present (Docker / direct env config).
if (typeof window !== "undefined" && process.env.NEXT_PUBLIC_AZURE_CLIENT_ID) {
  configureMsal(
    process.env.NEXT_PUBLIC_AZURE_CLIENT_ID,
    process.env.NEXT_PUBLIC_AZURE_TENANT_ID ?? "common",
  );
}
