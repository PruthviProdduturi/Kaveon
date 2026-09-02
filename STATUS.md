# Kaveon — Release Status

> Branch: `dev` (default)
> Module: Analyze module of the Kaveon suite
> Deploy: Vercel (kaveon-studio) + Azure Container Apps (kaveon-api) + Azure PostgreSQL (`kaveonmeta` + `kaveon`) · [kaveon.vercel.app](https://kaveon.vercel.app)

---

## Legend

| Symbol | Meaning |
|--------|---------|
| ✅ Done | Complete and verified |
| 🔄 In Progress | Active work, not yet verified end-to-end |
| 🧪 Alpha | Implemented for validation, not production-complete |
| 📋 Planning | Not started, scoped and understood |
| ❌ Blocked | Blocked by a dependency |

---

## Summary

| Area | Status |
|------|--------|
| Core analytics (datasets, charts, dashboards, SQL Lab) | ✅ Done |
| Authentication (GitHub / Google / Entra) + RBAC | 🔄 In Progress |
| Multi-source connectors | 🔄 In Progress |
| DLM (no-LLM NL→SQL, answer-from-context) — primary homepage path | ✅ Done |
| Metadata/data DB split (`kaveonmeta` + `kaveon`) | ✅ Done |
| CI/CD + repo standards | 🔄 In Progress |
| Superset-parity gaps | 📋 Planning |

## Kaveon Engine

| Capability | Status | Notes |
|---|---|---|
| Local Parquet reader, projection, metadata statistics, row-group pruning | 🧪 Alpha | Synchronous Arrow `RecordBatch` stream |
| Scan, filter, project, hash aggregate, limit | 🧪 Alpha | Vectorized node-local execution |
| SQL parser, CLI, HTTP statement API, catalog | 🧪 Alpha | Engine is not yet wired into Studio/API |
| Coordinator/worker discovery and heartbeats | 🧪 Alpha | Discovery only; queries still execute on the receiving node |
| Sort and TopN | 📋 Not Started | `ORDER BY` is parsed but not physically executed |
| Filter-pushdown optimizer pass | 📋 Not Started | Optimizer currently returns the input plan unchanged |
| ADLS Gen2 / S3 readers | 📋 Planning | Local validation precedes cloud object storage |
| Delta Lake / Iceberg readers | 📋 Planning | Snapshot and manifest semantics not implemented |
| Distributed scheduling, exchange, retry | 📋 Planning | Required before Trino-class distributed claims |
| Engine HTTP auth and TLS | 📋 Planning | Do not expose the alpha server directly to untrusted networks |

---

## Platform

| Item | Status | Notes |
|------|--------|-------|
| Semantic datasets — star schema, dimensions, metrics, role-playing dims (COALESCE) | ✅ Done | |
| Chart builder — 37 ECharts types incl. 3D WebGL globe | ✅ Done | |
| DLM — deterministic NL→SQL and precomputed answer context | ✅ Done | Supported question classes use the DLM; `nlToSql` is fallback |
| DLM serve-chart — single + multi-metric from precomputed context | ✅ Done | Coverage depends on the compiled dataset context |
| DLM filter values — dimension dropdown values from context | ✅ Done | Avoids source scans when the requested values are compiled |
| HLL sketch cuboids — mergeable COUNT(DISTINCT) over compiled dimension cuboids | ✅ Done | Approximate NDV; coverage and error depend on the compiled sketch |
| Star-schema semantic datasets | ✅ Done | Environment-specific scale belongs in reproducible benchmark manifests |
| Dashboard builder — drag-drop, rows/columns/tabs/text/headers/dividers | ✅ Done | Flat layout |
| Cross-filtering (click chart → filters others) | ✅ Done | |
| Dashboard filter bar — DLM-powered dropdowns, cascading filters, click-outside close | ✅ Done | Dimension count is dataset- and dashboard-specific |
| Client-side query cache — SHA-based dedup of repeated dashboard chart queries | ✅ Done | |
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
| Identity forwarded to the API via shared-secret-authenticated proxy headers (`KAVEON_PROXY_SECRET`) | ✅ Done | Static secret comparison, not per-request signing |
| RBAC — Viewer < Analyst < Editor < Admin | ✅ Done | |
| Content visibility — private / internal / published | ✅ Done | |
| Provider secrets encrypted at rest (Fernet/AES) | 🔄 In Progress | Connection strings remain plaintext in `data_sources`; no vault yet |
| Security headers, parameterization, proxy identity | 🔄 In Progress | Error-detail auditing remains open; see SECURITY.md |

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
| Platform CI (`.github/workflows/ci.yml`) | 🔄 In Progress | Checks exist, but frontend type/lint failures are not yet consistently blocking |
| Engine CI (`.github/workflows/engine.yml`) — format, Clippy, tests | ✅ Done | Rust warnings are denied |
| CD — Azure Container Apps and Vercel | ✅ Done | `.github/workflows/deploy.yml` deploys API; `ci.yml` deploys Studio on `dev` pushes after the web job |
| Vercel app config (`studio/vercel.json`) | ✅ Done |
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
| Engine server has no auth/TLS | Keep behind a trusted local boundary during alpha |

---

## Critical Gaps vs Superset (Planning)

1. Multi-auth beyond Azure/Google/Local (OAuth/SAML/LDAP)
2. Alerts & scheduled reports
3. Dashboard embedding (guest tokens)
4. Import/export dashboards/charts/datasets
5. Jinja SQL templating
6. Dataset certification / governance
7. ~~Advanced filters (multi-select, range, cascading)~~ → ✅ Done (multi-select, cascading from DLM context)
8. Non-Azure connectors (Snowflake, BigQuery, Databricks, Redshift)
