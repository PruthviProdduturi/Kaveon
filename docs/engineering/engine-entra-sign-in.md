# Microsoft Entra sign-in for the Engine

Status: implementation and automated validation complete; existing Entra application
configured for this test deployment. The user supplied client ID
`d0ce7c35-cc10-4ae7-b6be-60d002f43059` (Forge-Dev), owned by the deploying user.
Added the Kaveon delegated permission and SPA redirects while preserving Forge and
Trino scopes, existing redirects and token version 2. The ID is an application ID,
not a Service Tree ID. New-app creation was rejected by Microsoft tenant Service Tree
requirements; using the authorized existing app resolves that registration blocker.
Interactive Microsoft sign-in/consent still needs validation with the actual user.

This is a reusable configuration pattern, not a dependency on Forge-Dev. Another
deployment supplies its own tenant, approved app/client ID, SPA redirect URIs and
user object-ID role map. A separate app registration per deployment is preferable
for isolation. Service Tree ownership is specific to Microsoft's corporate tenant;
other tenants enforce their own registration/consent policies. Engine authorization
is separate from Azure subscription/AKS role assignments.

## Identity contract

The dashboard uses locally vendored MSAL Browser 4.30.0 and popup authorization
code flow with PKCE. It requests a delegated Engine access token, keeps tokens and
temporary authentication state in memory, renews silently while possible, and
clears the local session on Disconnect. The token form is an advanced alternative.
See Microsoft's [authorization code flow documentation](https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-auth-code-flow).

`GET /v1/auth/config` exposes only the configured tenant ID, client ID and scope.
The sign-in HTML and pinned MSAL script are public GET assets; Engine data routes
remain protected. Static principal, bridge, catalog and exchange credentials retain
their separate existing roles. An Entra identity is `entra:<tenant-id>:<object-id>`.

Server validation requires an RS256 signature from the configured tenant's Microsoft
JWKS, exact v2 issuer, Engine client-ID audience, matching tenant ID, valid expiry
and not-before, delegated `access_as_user` scope, and an explicitly allowed user
object ID. The configured object-ID map assigns reader/analyst/admin roles; signing
in does not grant a role automatically. Graph, Azure Resource Manager and AKS tokens
are not Engine tokens. Key retrieval is cached, bounded and throttled; unavailable
or stale key state fails closed.

## Required registration

In tenant `72f988bf-86f1-41af-91ab-2d7cd011db47`, create a single-tenant app named
`Kaveon Engine Test - prproddu` with the correct Service Tree ownership reference.
Set `api.requestedAccessTokenVersion` to 2 and expose a delegated permission named
`access_as_user` with identifier URI `api://<client-id>`. Configure these **SPA**
redirect URIs, not Web/confidential-client redirects:

- `https://localhost:8080/ui`
- `https://localhost:18443/ui`

For this deployment, use the existing client ID above; do not resubmit the failed
new-app request in `tmp/kaveon-entra-app-request.json`. For a new application in a
tenant requiring Service Tree ownership, add the approved `serviceManagementReference`.
After creation, update its identifier URI using the returned client ID and ensure
the delegated permission has the consent required by the tenant. No client secret
is needed in the browser. A tenant administrator may need to grant consent;
subscription Contributor access is not directory app-registration/consent authority.

## Engine configuration

Add this object to the existing `KAVEON_SECURITY_JSON`, preserving all existing
principal and bridge credentials. Supply the real client ID; do not deploy placeholders:

```json
{
  "entra": {
    "tenant_id": "72f988bf-86f1-41af-91ab-2d7cd011db47",
    "client_id": "<registered-engine-client-id>",
    "required_scope": "access_as_user",
    "principals": {
      "f85d5690-fe8d-463a-a51c-589baff5dd23": "admin"
    }
  }
}
```

For this AKS deployment, the coordinator uses Secret `kaveon-coordinator-auth`,
selected by Helm value `coordinator.credentialsSecret`. Update its `security.json`
key. Workers retain the original `kaveon-engine-auth` Secret and existing credentials;
they do not need interactive user authentication configuration.
Use a private file and `kubectl apply --server-side`; never print existing Secret
values or commit them. Deploy an image containing this implementation and restart
the coordinator to load the configuration. Internal worker/exchange credentials
are unchanged. Confirm `/v1/auth/config` returns the expected public configuration,
then open `/ui`, select **Sign in with Microsoft**, and test the actual assigned
account. Confirm an unassigned account is rejected. Keep static admin access until
this real sign-in/consent path is verified.

The optional Run Command viewer remains a static-token diagnostic tool and explicitly
returns `entra:null`. Native Microsoft sign-in is intended for the Engine's HTTPS
endpoint through the now-working port-forward. Kubectl credentials are never copied
to the browser.

## Validation

The CLI's default authentication tries the current Azure CLI session before device
sign-in, acquiring a token for this Engine's delegated scope and configured tenant.
Administrators can preauthorize Microsoft Azure CLI application
`04b07795-8ddb-461a-bbee-02f9e1bf7b46` for `access_as_user` on the Engine API app.
Existing tenant authentication requirements still apply; a fresh scoped Azure
login may be required. This application-level setting was added to the existing
test app without changing subscription policies or Engine user-role assignments.

Signed-token server tests cover accepted tokens and wrong issuer/audience/tenant,
expiry/not-before, missing scope/identity, unassigned users, algorithm and signature
failures, plus key-cache behavior. Existing HTTP security tests protect public assets
and authenticated data boundaries.

`python engine/qualification/ui_auth_browser.py` uses installed Microsoft Edge and
Python Playwright with a mocked identity provider. It verifies Microsoft sign-in,
the delegated Authorization header, memory-only cache, Disconnect, token-renewal
failure and 401 polling cancellation. This is not an interactive Entra sign-in test.

The deployed coordinator also passed TLS verification with its public CA and a
real Edge/MSAL smoke check that opened Microsoft's sign-in page. The configured
application's device-authorization endpoint accepted the CLI scope. Completing
interactive user sign-in and tenant consent remains a user validation step.
