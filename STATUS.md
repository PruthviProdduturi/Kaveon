# Kaveon — Release Status

> Branch: `dev` (default)
> Module: Analyze module of the Kaveon suite
> Deploy: Vercel (kaveon-web) + Render (kaveon-api) + Neon (Postgres) · [lens-analytics.vercel.app](https://lens-analytics.vercel.app)

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
| Multi-provider auth + RBAC | ✅ Done |
| Multi-source connectors | 🔄 In Progress |
| AI assistant (NL→SQL) | ✅ Done |
| CI/CD + repo standards | ✅ Done |
| Superset-parity gaps | 📋 Planning |

---

## Platform

| Item | Status | Notes |
|------|--------|-------|
| Semantic datasets — star schema, dimensions, metrics, role-playing dims (COALESCE) | ✅ Done | |
| Chart builder — 20+ ECharts types incl. 3D WebGL globe | ✅ Done | |
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
| Multi-provider auth — Local / Azure AD / Google, runtime-switchable from UI | ✅ Done | |
| JWT verification — RS256/JWKS (Azure AD, Google) + HS256 (local) | ✅ Done | |
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
| Trino | 📋 Planning |
| StarRocks | 📋 Planning |

## Repo Standards (Forge parity)

| Item | Status |
|------|--------|
| CI with gates (`.github/workflows/ci.yml`) — web lint/type-check/build, API syntax/tests, secret scan | ✅ Done |
| CD — Vercel (auto-deploy `dev`) + Render Blueprint (`render.yaml`) | ✅ Done |
| Vercel app config (`apps/kaveon-web/vercel.json`) | ✅ Done |
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
8. More chart types (Sankey, Treemap, Waterfall, Mixed time-series)
9. Non-Azure connectors (Snowflake, BigQuery, Databricks, Redshift)
