"use client";

import { PublicClientApplication } from "@azure/msal-browser";

export async function preparePublicEntra(config: { clientId: string; tenantId: string; scope: string }) {
  // Keep the API permission alongside the OIDC claims MSAL uses to establish
  // the signed-in account.  The identity scopes are harmless when already
  // granted and avoid an opaque interaction failure on first consent.
  const scopes = Array.from(new Set(["openid", "profile", "email", config.scope]));
  const client = new PublicClientApplication({
    auth: {
      clientId: config.clientId,
      authority: `https://login.microsoftonline.com/${config.tenantId}`,
      redirectUri: `${window.location.origin}/auth/microsoft`,
    },
    cache: { cacheLocation: "memoryStorage" },
  });
  await client.initialize();
  return async () => {
    const result = await client.loginPopup({ scopes, prompt: "select_account" });
    return result.accessToken;
  };
}
