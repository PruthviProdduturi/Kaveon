import { Configuration, PublicClientApplication } from "@azure/msal-browser";

const clientId = process.env.NEXT_PUBLIC_AZURE_CLIENT_ID ?? "";
const tenantId = process.env.NEXT_PUBLIC_AZURE_TENANT_ID ?? "common";
const redirectUri = process.env.NEXT_PUBLIC_AZURE_REDIRECT_URI || (typeof window !== 'undefined' ? window.location.origin : "");
const apiScope = process.env.NEXT_PUBLIC_AZURE_API_SCOPE;

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
