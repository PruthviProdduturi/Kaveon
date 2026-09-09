"use client";

import { PublicClientApplication } from "@azure/msal-browser";

export async function preparePublicEntra(config: { clientId: string; tenantId: string; scope: string }) {
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
    const result = await client.loginPopup({ scopes: [config.scope], prompt: "select_account" });
    return result.accessToken;
  };
}
