# Security Policy — Kaveon

## Overview

Kaveon is an **open-source, self-hosted analytics platform**. You run it on your own
infrastructure and point it at your own databases; there's also a public demo at
[kaveon.vercel.app](https://kaveon.vercel.app). It's built to be deployed however you
like — a private internal tool *or* a public site — so the security model assumes an
untrusted internet and defends accordingly.

Sign-in is **OAuth only** (GitHub, Google, Microsoft Entra ID) via NextAuth (Auth.js v5).
There are no local passwords. The browser only ever talks to the Next.js app; the FastAPI
backend is never exposed directly, and it trusts identity only from the web app's signed
proxy. Kaveon queries many databases — Microsoft Fabric SQL, Azure SQL, PostgreSQL, MySQL,
and StarRocks — not just one.

---

## Threat Model

| Asset | Threat | Mitigation |
|-------|--------|-----------|
| Connected data sources | Unauthorized data access | Every API endpoint requires an authenticated user; per-user scoping on user-owned data |
| User query history | Cross-user data leak | Records stored with `executed_by`; the API filters by the current user |
| API endpoints | Direct/unauthenticated access | API accepts requests only from the web proxy, which stamps `X-User-*` headers signed with `KAVEON_PROXY_SECRET`; mismatched/missing secret ⇒ rejected |
| Identity spoofing | Forged `X-User-*` headers | The API trusts `X-User-*` **only** when `KAVEON_PROXY_SECRET` matches; the browser never sends them directly |
| SQL query generation | Identifier injection | `quote_identifier()` wraps every user-supplied identifier and escapes it |
| Filter values | Operator injection | Filter operators validated against a strict allowlist in `query_generator.py` |
| Internal errors | Information disclosure | Exception details are logged server-side only; clients receive a generic message |
| Credential exposure | Connection-string leak | The `connection_string` column is excluded from every API response (`_PUBLIC_FIELDS`) |
| Schema enumeration | Data discovery without auth | All lab, SQL, and data-source endpoints require authentication |
| Content access | Reading others' private/internal content | Visibility model (`private` / `internal` / `published`) enforced in every list/get via `can_read()` in `middleware/permissions.py` |
| Privilege escalation | Lower role performing Editor/Admin actions | `require_min_role()` on all create/update/delete/admin endpoints |

---

## Authentication Architecture

Auth runs entirely in the Next.js app (NextAuth); the API trusts a signed proxy header —
the same pattern Forge uses. No tokens are handled in the browser.

```
Browser
   │  OAuth sign-in (GitHub / Google / Microsoft Entra ID) via NextAuth (Auth.js v5)
   │  session cookie, signed with AUTH_SECRET
   ▼
kaveon-studio  (Next.js)
   │  /api/kaveon/[...path] proxy — reads the session server-side and stamps:
   │     X-User-Email · X-User-Name · X-User-Role · X-Proxy-Secret (= KAVEON_PROXY_SECRET)
   ▼
kaveon-api  (FastAPI, middleware/auth.py)
   │  Trusts X-User-* ONLY when X-Proxy-Secret matches KAVEON_PROXY_SECRET
   │  The email in the header is the authoritative identity for all services
   │
   └─ require_min_role("Analyst"|"Editor"|"Admin")  (middleware/permissions.py)
         Grants/denies by role level; below minimum ⇒ 403 "forbidden"
```

The same `KAVEON_PROXY_SECRET` must be set on both `kaveon-studio` and `kaveon-api`.

> **Note:** the API retains a legacy Azure-AD/JWKS + `azure-identity` path used only when
> connecting to **Microsoft Fabric / Azure SQL** data sources via managed identity. It is
> not part of the sign-in flow, which is NextAuth OAuth end-to-end.

---

## Data Access Model

### Role-Based Access Control

The API defines four roles, ordered `Viewer < Analyst < Editor < Admin`. Through the
NextAuth sign-in, a user is **Admin** if their email is listed in `AUTH_ADMIN_EMAILS`,
otherwise **Viewer**; the Analyst/Editor tiers exist in the API's authorization layer for
finer-grained deployments.

| Role | Can read | Can create | Can publish | Can manage users/sources |
|---|---|---|---|---|
| Viewer | Published content | No | No | No |
| Analyst | Internal + published | Yes (own content) | No | No |
| Editor | Internal + published | Yes | Yes | No |
| Admin | All | Yes | Yes | Yes |

### Content Visibility

Every dataset, chart, and dashboard has a `visibility` field, enforced by a SQL clause
injected into every list/get query:

| Value | Readable by |
|---|---|
| `private` | Owner only |
| `internal` | Analyst, Editor, Admin |
| `published` | All authenticated users (including Viewers) |

### Other Scoped Resources

- **Query History** — scoped to `executed_by`; the workspace activity view intentionally aggregates the team's history.
- **Saved Queries / Favorites** — scoped per user.
- **Data Sources** — Admin-only to create/update/delete; `connection_string` is never returned in any API response.

---

## SQL Safety

All SQL sent to connected data sources uses one of two safe patterns:

1. **Parameterized queries** — service-layer database operations bind values as parameters (`?` / `@paramN`), never as embedded string literals.
2. **Quoted identifiers** — table/column/schema names in dynamic SQL pass through `quote_identifier()` in `services/query_generator.py`, which quotes each part and escapes the quote char.

Filter operators (`=`, `<`, `LIKE`, …) are validated against a strict allowlist before being embedded. A 64 KB size limit is enforced on all SQL-execution endpoints.

---

## Security Controls

| Control | Implementation |
|---------|---------------|
| **Proxy-authenticated API** | API trusts `X-User-*` only with a matching `KAVEON_PROXY_SECRET`; the browser never talks to the API directly |
| **OAuth sessions** | NextAuth (Auth.js v5) sessions signed with `AUTH_SECRET`; providers activate only when their client id/secret are set |
| **Authentication required** | Every non-setup endpoint requires an authenticated user |
| **Security headers** | `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, `Strict-Transport-Security`, `Referrer-Policy` added by middleware |
| **CORS hardening** | Explicit `allow_origins=[WEB_URL]`, explicit methods/headers — no wildcards |
| **Error message safety** | Handlers never return exception details to clients; full tracebacks to server logs only |
| **Credential suppression** | `connection_string` excluded from all data-source responses (`_PUBLIC_FIELDS`) |
| **SQL injection** | Parameterized queries throughout; `quote_identifier()` for dynamic identifiers; operator allowlist |
| **Setup endpoint gating** | Setup endpoints return 403 once the app is configured |
| **Query size limit** | 64 KB max on SQL-execution endpoints |
| **RBAC** | `require_min_role()` on all create/update/admin endpoints |
| **Content visibility** | `private` / `internal` / `published` enforced in every list/get query |

---

## Reporting Security Issues

Kaveon is a personal, open-source project. If you find a vulnerability, please **do not open
a public GitHub issue**. Instead, open a private
[GitHub Security Advisory](https://github.com/PruthviProdduturi/Kaveon/security/advisories/new)
or email the maintainer. Include a description, steps to reproduce, the affected
component(s), and the potential impact. Responsible disclosure is appreciated.

---

## Security Checklist for Contributors

Before merging a PR that touches API routes or services:

- [ ] User identity comes from the proxy-authenticated context, never a raw client header
- [ ] All SQL identifiers pass through `quote_identifier()`
- [ ] All SQL values are bound as parameters, not embedded literals
- [ ] Filter operators are validated against the allowlist in `query_generator.py`
- [ ] Error responses don't expose SQL errors, stack traces, or internal state
- [ ] SQL-execution endpoints enforce the 64 KB size limit
- [ ] New endpoints require an authenticated user unless explicitly public
- [ ] Create/update/delete endpoints use `require_min_role("Analyst")` or higher
- [ ] Visibility filtering is applied to any new list/get of user-owned objects
- [ ] Admin-only endpoints use `require_min_role("Admin")`

---

## Dependency Security

Keep security-relevant dependencies current:

| Package | Purpose |
|---------|---------|
| `next-auth` (Auth.js v5) | OAuth sign-in (GitHub / Google / Microsoft Entra) |
| `psycopg2-binary` / `PyMySQL` | Parameterized SQL for PostgreSQL / MySQL / StarRocks |
| `pyodbc` | Parameterized SQL over ODBC Driver 18 (Fabric / Azure SQL) |
| `azure-identity` | Managed-identity tokens for Fabric / Azure SQL sources (optional) |
| `cryptography` | Encrypts stored secrets (e.g. AI provider keys) at rest |
