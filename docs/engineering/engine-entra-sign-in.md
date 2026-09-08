# Microsoft Entra sign-in for the Engine

Status: implementation and automated validation complete; live tenant registration
and interactive sign-in are pending. The Microsoft tenant rejected app creation
with `ServiceTreeValueMissing`: a valid `serviceManagementReference` (Service Tree
ownership ID) is required. Do not invent an ownership ID or change tenant policies.
An approved existing application may be used only with authorization from its owner.

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

The initial request manifest is available locally in
`tmp/kaveon-entra-app-request.json`. Add the approved `serviceManagementReference`
before submitting it. The failed creation did not produce an application/client ID.
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

For this AKS chart, update the `security.json` key in Secret `kaveon-engine-auth`.
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

Signed-token server tests cover accepted tokens and wrong issuer/audience/tenant,
expiry/not-before, missing scope/identity, unassigned users, algorithm and signature
failures, plus key-cache behavior. Existing HTTP security tests protect public assets
and authenticated data boundaries.

`python engine/qualification/ui_auth_browser.py` uses installed Microsoft Edge and
Python Playwright with a mocked identity provider. It verifies Microsoft sign-in,
the delegated Authorization header, memory-only cache, Disconnect, token-renewal
failure and 401 polling cancellation. This is not an interactive Entra sign-in test.
