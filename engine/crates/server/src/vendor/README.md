# Microsoft authentication browser library

`msal-browser.min.js` is the unmodified browser bundle from
`@azure/msal-browser@4.30.0` (MIT; see `MSAL-LICENSE`). It is served locally at
`/ui/msal-browser.min.js`; dashboard sign-in does not execute CDN-hosted code.

SHA-256: `2b580165064a6a4ec041ebd44832299172688a23a42c3b81d65314799dba04eb`.

To reproduce, use `npm pack @azure/msal-browser@4.30.0` in a temporary directory,
extract the archive, and copy `package/lib/msal-browser.min.js` and
`package/LICENSE`. Preserve the version, license and digest when upgrading.

This uses the v4 popup authorization-code/PKCE flow. Both MSAL token cache and
temporary authentication state use `memoryStorage`; refresh closes the local
session. The popup returns to the registered SPA redirect URI at the dashboard
origin plus `/ui`. No client secret belongs in the browser. Engine access is
still authorized by the server; Microsoft sign-in does not grant engine roles.

References:
- https://learn.microsoft.com/en-us/entra/msal/javascript/browser/initialization
- https://learn.microsoft.com/en-us/entra/msal/javascript/browser/configuration
- https://github.com/AzureAD/microsoft-authentication-library-for-js/tree/msal-browser-v4.30.0/lib/msal-browser
