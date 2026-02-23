# Security Policy — LoomX

## Overview

LoomX is an internal enterprise data exploration platform built on top of Microsoft Fabric.
It is intended for use by authenticated Azure AD users within the organization and is
**not** designed to be exposed to the public internet.

---

## Threat Model

| Asset | Threat | Mitigation |
|-------|--------|-----------|
| Microsoft Fabric SQL databases | Unauthorized data access | Azure AD authentication; per-user data scoping |
| User query history | Privacy — cross-user data leak | Queries stored with `executed_by`; API filters by user |
| API endpoints | CSRF on state-changing operations | Bearer token required on all mutating calls via `msalFetch` |
| SQL query generation | Identifier injection | `quoteIdentifier()` wraps all user-supplied identifiers in `[...]` with `]` escaped as `]]` |
| Filter values | Operator injection | Operator values validated against a strict allowlist |
| User identity | Identity spoofing via header | `x-user-email` header is SECONDARY to JWT-extracted identity |

---

## Authentication Architecture

```
Browser (MSAL)
    │
    │  Authorization: Bearer <Azure AD access token>
    │  x-user-email: user@contoso.com  (secondary, legacy)
    ▼
Express API (loomx-api)
    │
    ├─ authMiddleware.extractUser
    │     Decodes JWT payload, validates exp + iss
    │     Sets req.user.email from preferred_username claim
    │
    ├─ Routes use req.user.email as authoritative identity
    │     Fallback to x-user-email only if no Bearer token
    │     Final fallback: 'anonymous' (never 'system')
    │
    └─ pythonProxyService → Python Flask → Microsoft Fabric ODBC
```

### Current Limitations

1. **JWT signature is not verified server-side.**
   The `authMiddleware` validates JWT structure, expiry, and issuer, but does not verify
   the cryptographic signature against Azure AD's JWKS endpoint.

   **Recommended next step:** Add `jsonwebtoken` + `jwks-rsa` packages (or use
   `@azure/msal-node` `ConfidentialClientApplication` with `AZURE_CLIENT_SECRET`) to
   perform full signature verification. This eliminates the ability for an attacker to
   forge a token with arbitrary claims.

2. **No server-side rate limiting.**
   The Python proxy and SQL execution endpoints have no per-user rate limits.

   **Recommended next step:** Add `express-rate-limit` middleware on SQL execution routes.

3. **SQL Lab executes arbitrary user SQL.**
   This is by design — SQL Lab is a developer tool for authenticated enterprise users.
   Queries are logged to `query_history` with the authenticated user's identity.

---

## Data Access Model

- **Charts, Dashboards, Datasets** — readable by all authenticated users; creation is attributed to the creator.
- **Query History** — each record is scoped to `executed_by` (user email). The workspace activity view aggregates all users' history; this is intentional for team visibility.
- **Saved Queries** — scoped per user; list/read/update/delete operations require matching `user_id`.
- **Favorites** — scoped per user.

---

## SQL Safety

All SQL sent to Microsoft Fabric is constructed using one of two safe patterns:

1. **Parameterized queries** — used by all service layer DB operations via `metadataProxyService.query()`.
   Parameters are passed as a separate array and bound by the Tedious driver.

2. **Quoted identifiers** — table names, column names, and schema names used in dynamic
   SQL (e.g., distinct values query, chart preview query) are passed through `quoteIdentifier()`,
   which wraps each part in `[...]` and escapes `]` as `]]`.

Filter operators (e.g., `=`, `<`, `LIKE`) are validated against a strict allowlist in
`queryGenerator.service.ts` before being embedded in SQL.

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

- [ ] User identity is read from `req.user.email` (not from a client-supplied header)
- [ ] All SQL identifiers (table names, column names) pass through `quoteIdentifier()`
- [ ] All SQL values are bound as parameters, not embedded as string literals
- [ ] Filter operators are validated against the `SAFE_OPERATORS` allowlist
- [ ] State-changing frontend fetch calls use `msalFetch` (not bare `fetch`)
- [ ] Error responses in production do not expose SQL error messages or stack traces
- [ ] New query execution endpoints enforce the 64 KB query size limit

---

## Dependency Security

Keep dependencies up to date. The critical security-relevant packages are:

| Package | Purpose |
|---------|---------|
| `helmet` | HTTP security headers |
| `cors` | CORS origin restriction |
| `@azure/msal-browser` | Frontend MSAL authentication |
| `@azure/msal-node` | Backend Azure AD integration |
| `@azure/identity` | Azure identity primitives |
| `tedious` | Microsoft SQL Server parameterized queries |
