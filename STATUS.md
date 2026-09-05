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
| CI/CD + repo standards | ✅ Done |
| Superset-parity gaps | 📋 Planning |

## Kaveon Engine

| Capability | Status | Notes |
|---|---|---|
| Local Parquet reader, projection, metadata statistics, row-group pruning | 🧪 Alpha | Synchronous Arrow `RecordBatch` stream |
| Local Delta Lake reader | 🧪 Alpha | Replays complete JSON commits from version 0 and streams all active Parquet files; checkpoints unsupported |
| Scan, filter, project, hash aggregate, limit | 🧪 Alpha | Vectorized node-local execution |
| SQL coverage — window functions, set operations, date/time, decimal, subqueries | 🧪 Alpha | ROW_NUMBER/RANK/DENSE_RANK/LAG/LEAD/SUM/AVG/COUNT/MIN/MAX OVER with frame specs (ROWS/RANGE/GROUPS BETWEEN), INTERSECT/EXCEPT, EXTRACT, DATE_TRUNC, DATE_PART, TO_CHAR, NOW/CURRENT_DATE/CURRENT_TIMESTAMP; DISTINCT on SUM/AVG; Decimal128 type; IN/NOT IN/EXISTS/NOT EXISTS subqueries via semi/anti join |
| SQL parser, remote-first CLI, HTTP statement API | 🧪 Alpha | Engine query execution is not yet wired into Studio/API identity |
| Native durable catalog | 🧪 Alpha | SQLite/WAL single-coordinator authority with revisions, lifecycle, audit, Arrow schemas, authenticated mutation API, and restart recovery |
| Coordinator/worker discovery and heartbeats | 🧪 Alpha | Two-worker local Docker topology verified on real Delta data |
| Query lifecycle and storage-scan telemetry | 🧪 Alpha | HTTP history retains real phase timings, logical plans, file/row-group pruning, selected bytes, emitted rows, and scan throughput |
| Physical operator, stage, and task telemetry | 🧪 Alpha | Completed distributed stages/tasks report worker, partition, elapsed time, rows, Arrow batches, and bytes; live and per-operator metrics remain target |
| Sort and TopN | 🧪 Alpha | Local and distributed execution with fixed-fan-in external merge |
| Filter-pushdown optimizer pass | 🧪 Alpha | Wired conservative pushdown with residual row-level evaluation |
| ADLS Gen2 / S3 readers | 📋 Planning | Local validation precedes cloud object storage |
| Cloud Delta Lake / Iceberg readers | 📋 Planning | ADLS/S3 Delta, checkpoint replay, and Iceberg manifest semantics not implemented |
| Distributed scheduling, exchange, retry | 🧪 Alpha | Versioned fragments execute scans, partial/final aggregates including AVG/exact DISTINCT, Sort/TopN, and repartitioned/broadcast joins; retry/cancellation wired |
| Hash exchange | 🧪 Alpha | Authenticated Arrow IPC v2, stable partitioning, bounded payloads, idempotent upload, and cleanup; streaming flow control remains target |
| Engine HTTP auth and TLS | 📋 Planning | Internal exchange/catalog mutation tokens exist; end-user statements still require a trusted boundary |

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
| Platform CI (`.github/workflows/ci.yml`) | ✅ Done | Frozen dependency install, documentation validation, type checking, linting, tests, and production build are blocking gates |
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
| Platform catalog-source registry is not synchronized to Engine catalog | Add an authenticated revision-aware API-to-Engine bridge |
| Engine aggregate/join memory is bounded only through opt-in constructors | Wire admission and accounts through every coordinator/worker plan, then add partitioned aggregate/join spill before production scale claims |

---

## Critical Gaps vs Superset (Planning)

1. Enterprise identity beyond the configured OAuth providers (SAML/LDAP)
2. Alerts & scheduled reports
3. Dashboard embedding (guest tokens)
4. Import/export dashboards/charts/datasets
5. Jinja SQL templating
6. Dataset certification / governance
7. ~~Advanced filters (multi-select, range, cascading)~~ → ✅ Done (multi-select, cascading from DLM context)
8. Non-Azure connectors (Snowflake, BigQuery, Databricks, Redshift)
