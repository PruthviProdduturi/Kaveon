# Security Policy — LoomX

## Overview

LoomX is an internal enterprise data exploration platform built on top of Microsoft Fabric.
It is intended for use by authenticated Azure AD users within the organization and is
**not** designed to be exposed to the public internet.

---

## Threat Model

| Asset | Threat | Mitigation |
|-------|--------|-----------|
| Microsoft Fabric SQL databases | Unauthorized data access | Azure AD JWT verification (RS256); per-user data scoping |
| User query history | Privacy — cross-user data leak | Queries stored with `executed_by`; API filters by user |
| API endpoints | CSRF on state-changing operations | Bearer token required on all endpoints via `require_auth` dependency |
| SQL query generation | Identifier injection | `quote_identifier()` wraps all user-supplied identifiers in `[...]` with `]` escaped as `]]` |
| Filter values | Operator injection | Operator values validated against a strict allowlist in `query_generator.py` |
| User identity | Identity spoofing via header | `x-user-email` header is **rejected** when AAD is configured; only a valid signed Bearer JWT is accepted |
| Internal errors | Information disclosure | Exception details are logged server-side only; clients always receive a generic error message |
| Credential exposure | Connection string leak | `connection_string` column is excluded from all API responses via `_PUBLIC_FIELDS` constant |
| Unauthenticated schema enumeration | Data discovery without auth | All lab, SQL, and data-source endpoints require `require_auth` dependency |
| Content access | Unauthorized read of private/internal data | Visibility model (private/internal/published) enforced in all list/get service calls; `can_read()` helper in `middleware/permissions.py` |
| Privilege escalation | Lower-role user performing Editor/Admin actions | `require_min_role()` dependency on all create/update/delete/admin endpoints; role resolved from JWT claim → DB → default Viewer |

---

## Authentication Architecture

```
Browser (MSAL)
    │
    │  Authorization: Bearer <Azure AD access token (RS256 signed)>
    ▼
FastAPI (loomx-api)
    │
    ├─ middleware/auth.py — get_current_user()
    │     When AZURE_CLIENT_ID + AZURE_TENANT_ID are set (production):
    │       Fetches Azure AD JWKS public keys (cached by PyJWKClient)
    │       Verifies RS256 signature, expiry, and audience
    │       Extracts email from preferred_username / email / upn claim
    │
    │     When AAD is NOT configured (first-run setup mode only):
    │       Falls back to unverified JWT decode OR x-user-email header
    │       This is intentional — no metadata DB exists yet in setup mode
    │
    ├─ require_auth dependency — applied to every protected endpoint
    │     Returns 401 if no authenticated user is present
    │     User email is the authoritative identity throughout all services
    │
    ├─ require_user_context(ctx) dependency
    │     → Returns UserContext(email, role, jwt_roles) if present
    │     → Raises HTTP 401 if None
    │     → Role resolved by users_svc.resolve_role() (JWT only; None → 403 for oauth)
    │
    └─ require_min_role("Analyst") dependency factory (middleware/permissions.py)
          → Calls require_user_context internally
          → Returns UserContext if role level ≥ minimum
          → Raises HTTP 403 with code "forbidden" if below minimum

    └─ database/pool.py — FabricSQLConnection
          Uses DefaultAzureCredential (Managed Identity in production)
          Injects token via SQL_COPT_SS_ACCESS_TOKEN (no SQL username/password)
          Connects directly to Microsoft Fabric SQL over ODBC/TLS
```

---

## Data Access Model

### Role-Based Access Control

All authenticated users are assigned one of four roles from JWT claims only:
1. Azure AD App Role claim (`roles[]` in the JWT) — highest matching role wins
2. Azure AD/Google users with no App Role → **NoAccess** (403, sign-out screen shown)
3. Local auth users with no JWT role → **Viewer** default (dev fallback)

| Role | Can read | Can create | Can publish | Can manage users/sources |
|---|---|---|---|---|
| Viewer | Published content only | No | No | No |
| Analyst | Internal + published | Yes (own content) | No | No |
| Editor | Internal + published | Yes | Yes | No |
| Admin | All | Yes | Yes | Yes |

### Content Visibility

Every dataset, chart, and dashboard has a `visibility` field:

| Value | Readable by |
|---|---|
| `private` | Owner only |
| `internal` | Analyst, Editor, Admin |
| `published` | All authenticated users (including Viewers) |

Visibility is enforced in `services/datasets.py`, `services/charts.py`, and `services/dashboards.py` via a `_vis_clause()` SQL fragment injected into every list and get query.

### Other Scoped Resources

- **Query History** — each record is scoped to `executed_by`. The workspace activity view aggregates all users' history; this is intentional for team visibility.
- **Saved Queries** — scoped per user; list/read/update/delete require matching `user_id`.
- **Favorites** — scoped per user.
- **Data Sources** — Admin-only for create/update/delete; readable by all authenticated users. `connection_string` is never returned in any API response.

---

## SQL Safety

All SQL sent to Microsoft Fabric is constructed using one of two safe patterns:

1. **Parameterized queries** — used by all service-layer database operations via `database/metadata.py` and `pool.execute_query()`. Parameters are passed as a separate list and bound by `pyodbc` using `?` or `@paramN` placeholders, never embedded as string literals.

2. **Quoted identifiers** — table names, column names, and schema names used in dynamic SQL (e.g., distinct values query, chart preview query) are passed through `quote_identifier()` in `services/query_generator.py`, which wraps each part in `[...]` and escapes `]` as `]]`.

Filter operators (e.g., `=`, `<`, `LIKE`) are validated against a strict allowlist in `services/query_generator.py` before being embedded in SQL.

---

## Security Controls (S360 Compliance)

| Control | Implementation |
|---------|---------------|
| **JWT signature verification** | RS256 via PyJWT + PyJWKClient; Azure AD JWKS endpoint; audience checked against `AZURE_CLIENT_ID` |
| **Authentication required** | `require_auth` FastAPI dependency on every non-setup endpoint |
| **Security headers** | `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, `Strict-Transport-Security`, `Referrer-Policy`, `X-Permitted-Cross-Domain-Policies` added by middleware in `main.py` |
| **CORS hardening** | Explicit `allow_origins=[WEB_URL]`, explicit method list, explicit header list — no wildcards |
| **Error message safety** | `generic_error_handler` never returns exception details to clients; full traceback to server logs only |
| **Credential suppression** | `connection_string` excluded from all data-source SELECT queries via `_PUBLIC_FIELDS` constant |
| **SQL injection** | Parameterized queries throughout; `quote_identifier()` for dynamic identifiers |
| **Setup endpoint gating** | `POST /setup/test` and `POST /setup/initialize` return 403 once the app is configured |
| **Query size limit** | 64 KB maximum enforced on all SQL execution endpoints |
| **Role-based access control** | `require_min_role()` FastAPI dependency in `middleware/permissions.py`; role resolved from JWT → DB → default Viewer; applied on all create/update/admin endpoints |
| **Content visibility enforcement** | `_vis_clause()` SQL fragment in all dataset/chart/dashboard service list/get calls; private/internal/published model |

---

## Reporting Security Issues

If you discover a security vulnerability in LoomX, report it to the owning team via
the internal security disclosure channel. Do not file public GitHub issues for
security-sensitive findings.

Include:
- Description of the vulnerability
- Steps to reproduce
- Affected component(s)
- Potential impact

---

## Security Checklist for Contributors

Before merging a PR that touches API routes or services, verify:

- [ ] User identity is read from the `require_auth` dependency result (not from a client-supplied header)
- [ ] All SQL identifiers (table names, column names) pass through `quote_identifier()`
- [ ] All SQL values are bound as pyodbc parameters, not embedded as string literals
- [ ] Filter operators are validated against the `SAFE_OPERATORS` allowlist in `query_generator.py`
- [ ] State-changing frontend fetch calls use `msalFetch` (not bare `fetch`)
- [ ] Error responses do not expose SQL error messages, stack traces, or internal state
- [ ] New query execution endpoints enforce the 64 KB query size limit
- [ ] New endpoints include `user: str = Depends(require_auth)` unless explicitly public
- [ ] New create/update/delete endpoints use `require_min_role("Analyst")` or higher, not just `require_auth`
- [ ] Visibility filtering (`_vis_clause`) is applied in any new list or get query for user-owned objects
- [ ] New Admin-only endpoints use `require_min_role("Admin")` dependency

---

## Dependency Security

Keep dependencies up to date. The critical security-relevant packages are:

| Package | Purpose |
|---------|---------|
| `PyJWT` | JWT decoding and RS256 signature verification |
| `cryptography` | RSA key operations (used by PyJWT for RS256) |
| `azure-identity` | DefaultAzureCredential / Managed Identity for Fabric SQL |
| `@azure/msal-browser` | Frontend MSAL authentication |
| `pyodbc` | Parameterized SQL execution via ODBC Driver 18 |
