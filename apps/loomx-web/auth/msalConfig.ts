import { Configuration, PublicClientApplication } from "@azure/msal-browser";

const clientId = process.env.NEXT_PUBLIC_AAD_CLIENT_ID || "cd692cda-e48c-4f9b-9419-d6fc96ee506d";
const tenantId = process.env.NEXT_PUBLIC_AAD_TENANT_ID || "11111111-1111-4111-8111-111111111111";
const redirectUri = process.env.NEXT_PUBLIC_AAD_REDIRECT_URI || (typeof window !== 'undefined' ? window.location.origin : "");
const apiScope = process.env.NEXT_PUBLIC_AAD_API_SCOPE;

export const msalConfig: Configuration = {
  auth: {
    clientId,
    authority: `https://login.microsoftonline.com/${tenantId}`,
    redirectUri,
  },
  cache: {
    cacheLocation: "sessionStorage",
  },
  system: {
    allowRedirectInIframe: false,
  },
};

export const loginRequest = {
  scopes: apiScope ? [apiScope] : ["User.Read"],
};

export const msalInstance = new PublicClientApplication(msalConfig);
