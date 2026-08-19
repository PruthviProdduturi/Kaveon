# Kaveon — Release Status

> Branch: `dev` (default)
> Module: Analyze module of the Kaveon suite
> Deploy: Vercel (kaveon-web) + Azure Container Apps (kaveon-api) + Azure PostgreSQL (`kaveonmeta` + `kaveon`) · [kaveon.vercel.app](https://kaveon.vercel.app)

---

## Legend

| Symbol | Meaning |
|--------|---------|
| ✅ Done | Complete and verified |
| 🔄 In Progress | Active work, not yet verified end-to-end |
| 📋 Planning | Not started, scoped and understood |
| ❌ Blocked | Blocked by a dependency |

---

## Summary

| Area | Status |
|------|--------|
| Core analytics (datasets, charts, dashboards, SQL Lab) | ✅ Done |
| OAuth auth (GitHub / Google / Microsoft Entra) + RBAC | ✅ Done |
| Multi-source connectors | 🔄 In Progress |
| DLM (no-LLM NL→SQL, answer-from-context) — primary homepage path | ✅ Done |
| Metadata/data DB split (`kaveonmeta` + `kaveon`) | ✅ Done |
| CI/CD + repo standards | ✅ Done |
| Superset-parity gaps | 📋 Planning |

---

## Platform

| Item | Status | Notes |
|------|--------|-------|
| Semantic datasets — star schema, dimensions, metrics, role-playing dims (COALESCE) | ✅ Done | |
| Chart builder — 37 ECharts types incl. 3D WebGL globe | ✅ Done | |
| DLM — no-LLM NL→SQL, precomputed answer-from-context (10M rows → ~1.5s, no scan) | ✅ Done | Primary homepage path; `nlToSql` is fallback |
| Dashboard builder — drag-drop, rows/columns/tabs/text/headers/dividers | ✅ Done | Flat layout |
| Cross-filtering (click chart → filters others) | ✅ Done | |
| SQL Lab — Monaco, multi-tab, history, saved queries | ✅ Done | |
| Async query execution + result caching (SHA-256, TTL) | ✅ Done | Async job store not cleaned on restart |
| CTAS endpoint | ✅ Done | |
| Per-user favorites, themes, workspace activity | ✅ Done | |
| First-run setup wizard (metadata DB) | ✅ Done | |
| Rate limiting (120 SQL/user/min; Redis-backed if `REDIS_URL`) | ✅ Done | In-process limiter not shared across replicas |
| Connection pool warmup + 5-min heartbeat | ✅ Done | |

## Auth & Security

| Item | Status | Notes |
|------|--------|-------|
| OAuth sign-in — GitHub / Google / Microsoft Entra via NextAuth (Auth.js v5) | ✅ Done | |
| Identity forwarded to the API via signed proxy headers (KAVEON_PROXY_SECRET) | ✅ Done | |
| RBAC — Viewer < Analyst < Editor < Admin | ✅ Done | |
| Content visibility — private / internal / published | ✅ Done | |
| Secrets encrypted at rest (Fernet/AES) | ✅ Done | Connection strings still plaintext in `data_sources` — no vault yet |
| S360 controls (headers, CORS, param queries, error safety) | ✅ Done | See SECURITY.md |

## Connectors

| Source | Status |
|--------|--------|
| Fabric SQL | ✅ Done |
| Azure SQL | ✅ Done |
| PostgreSQL | ✅ Done |
| MySQL | ✅ Done |
| StarRocks (MySQL protocol) | ✅ Done |
| Trino | 📋 Planning (no driver yet) |

## Repo Standards (Forge parity)

| Item | Status |
|------|--------|
| CI with gates (`.github/workflows/ci.yml`) — web lint/type-check/build, API syntax/tests, secret scan | ✅ Done |
| CD — Vercel (auto-deploy `dev`) + Azure Container Apps (Bicep IaC in `infra/bicep/`) | ✅ Done |
| Vercel app config (`apps/kaveon-web/vercel.json`) | ✅ Done |
| Bicep IaC (ACR, Container Apps, PostgreSQL, Key Vault, Log Analytics) | ✅ Done |
| CONTRIBUTING.md · SECURITY.md · LICENSE · ARCHITECTURE.md · DEPLOYMENT.md | ✅ Done |
| PR template · issue templates (bug/feature) | ✅ Done |
| `.gitleaksignore` · `.dockerignore` · `.env.example` | ✅ Done |
| STATUS.md — this file | ✅ Done |

---

## Known Issues

| Issue | Notes |
|-------|-------|
| Charts schema uses INT PK but some paths treat id as NVARCHAR(36) | Reconcile |
| Async job store not cleaned on restart | Add cleanup on startup |
| Connection strings stored plaintext in `data_sources` | Needs vault integration |
| In-process rate limiting not shared across replicas | Redis fixes this |

---

## Critical Gaps vs Superset (Planning)

1. Multi-auth beyond Azure/Google/Local (OAuth/SAML/LDAP)
2. Alerts & scheduled reports
3. Dashboard embedding (guest tokens)
4. Import/export dashboards/charts/datasets
5. Jinja SQL templating
6. Dataset certification / governance
7. Advanced filters (multi-select, range, cascading)
8. Non-Azure connectors (Snowflake, BigQuery, Databricks, Redshift)
