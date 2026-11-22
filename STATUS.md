# Kaveon — Release Status

> Branch: `dev` (default)
> Module: Analyze module of the Kaveon suite
> Deploy: Vercel (kaveon-studio) + Azure Container Apps (kaveon-api) · [kaveon.io](https://kaveon.io)
> Control plane: the Engine's transaction API over object storage. **PostgreSQL
> is retired** — datasets, charts, dashboards, saved queries, favourites and
> query history are product records committed through the Engine, not rows in a
> relational database.
>
> Native installs default to the PostgreSQL-free path. Docker Compose sets
> `KAVEON_POSTGRESQL_RETIREMENT_MODE=true` for the default stack, and the Helm
> chart defaults `api.cutover.retirementMode: true` in
> `infra/helm/kaveon-platform/values.yaml`. The optional `local-postgres`
> Compose profile and retirement/reconciliation Jobs remain historical
> migration and rollback tooling; they are not application authority.

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
| KaveonDB product authority and lake catalogs | ✅ Done (local/native scope) | Product records use the Engine transaction API; external SQL systems remain optional passthrough sources; cloud recovery and multi-coordinator gates remain explicit |
| CI/CD + repo standards | ✅ Done |
| Superset-parity gaps | 📋 Planning |

## Kaveon Engine

| Capability | Status | Notes |
|---|---|---|
| Local Parquet reader, projection, metadata statistics, row-group pruning | ✅ Done (local scope) | Synchronous Arrow `RecordBatch` stream; exact metadata and pruning are qualified |
| Local Delta Lake reader | ✅ Done (local scope) | Pins a Delta snapshot, replays checkpoints/tails, handles tombstones and multipart completeness, and streams active Parquet files; advanced protocol features remain unsupported |
| Scan, filter, project, hash aggregate, limit | ✅ Done (local scope) | Vectorized execution with bounded memory and spill evidence |
| SQL coverage — window functions, set operations, date/time, decimal, subqueries | 🧪 Alpha | ROW_NUMBER/RANK/DENSE_RANK/LAG/LEAD/SUM/AVG/COUNT/MIN/MAX OVER with frame specs (ROWS/RANGE/GROUPS BETWEEN), INTERSECT/EXCEPT, EXTRACT, DATE_TRUNC, DATE_PART, TO_CHAR, NOW/CURRENT_DATE/CURRENT_TIMESTAMP; DISTINCT on SUM/AVG; Decimal128 type; IN/NOT IN/EXISTS/NOT EXISTS subqueries via semi/anti join |
| SQL parser, remote-first CLI, HTTP statement API | ✅ Done (local scope) | Studio/API routes use the authenticated Engine bridge; packaged CLI and API contracts are qualified |
| Native durable catalog | ✅ Done (local scope) | KaveonDB object-backed snapshots, revisions, lifecycle, audit, Arrow schemas, authenticated mutation API, and restart recovery |
| Coordinator/worker discovery and heartbeats | ✅ Done (local scope) | Two-worker local Docker topology verified on real Delta data |
| Query lifecycle and storage-scan telemetry | ✅ Done (local scope) | HTTP history retains phase timings, logical plans, pruning, selected bytes, emitted rows, scan throughput, and cache counters |
| Physical operator, stage, and task telemetry | ✅ Done (local scope) | Distributed stages/tasks report worker, partition, elapsed time, rows, Arrow batches, bytes, and bounded estimate attribution; deeper live operator metrics remain open |
| Sort and TopN | ✅ Done (local scope) | Local and distributed execution with bounded fixed-fan-in external merge |
| Filter-pushdown optimizer pass | ✅ Done (local scope) | Conservative pushdown with residual row-level evaluation |
| ADLS Gen2 / S3 readers | ✅ ADLS local qualification | ADLS Gen2 Parquet and Delta paths are qualified on declared datasets; S3 uses the shared object-store path but remains unqualified |
| Cloud Delta Lake / Iceberg readers | ✅ Delta local qualification | Delta is qualified on ADLS Gen2 paths; Iceberg is qualified locally but its ADLS deployment gate remains open |
| Distributed scheduling, exchange, retry | ✅ Done (local scope) | Versioned fragments execute scans, aggregates, Sort/TopN, and repartitioned/broadcast joins with retry and cancellation |
| Hash exchange | ✅ Done (local scope) | Authenticated Arrow IPC, stable partitioning, bounded payloads, idempotent upload, retries, fencing, and cleanup; cloud flow-control qualification remains open |
| Engine HTTP auth and TLS | ✅ Done (local scope) | Internal exchange/catalog mutation tokens and authenticated end-user statement routes are qualified locally; cloud identity rotation and deployment qualification remain open |

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
| Control plane on object storage — datasets, charts, dashboards, saved queries, favourites and history as product records committed through the Engine's transaction API | ✅ Done (local/native scope) | PostgreSQL retired. Records are published beside the catalog snapshot in content-addressed shards; capacity and restart/CAS behavior are qualified on in-memory, filesystem, and Azurite stores. Cloud recovery remains open |
| Async query execution + result caching (SHA-256, TTL) | ✅ Done | Async job store not cleaned on restart |
| CTAS endpoint | ✅ Done | |
| Per-user favorites, themes, workspace activity | ✅ Done | |
| First-run setup wizard (KaveonDB authority) | ✅ Done | Native setup no longer requires a PostgreSQL metadata database |
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
| Fabric SQL | ✅ Passthrough qualified | API ODBC routing is qualified; Kaveon Engine execution and cross-source joins are not claimed |
| Azure SQL | ✅ Passthrough qualified | API ODBC routing is qualified; Kaveon Engine execution and cross-source joins are not claimed |
| PostgreSQL | 🧭 Legacy / migration only | Not a KaveonDB authority; retained only for explicitly controlled migration or reconciliation windows |
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
| Bicep IaC (ACR, Container Apps, Key Vault, Log Analytics) | ✅ Done | The PostgreSQL module is retained only for historical migration/rehearsal resources and is not part of the native application stack |
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
| Engine deployment hardening | TLS/auth are implemented; keep the Engine behind the trusted API boundary while rotation, network controls and recovery qualification remain open |
| Platform catalog-source registry is not synchronized to Engine catalog | Add an authenticated revision-aware API-to-Engine bridge |
| Cloud-scale aggregate/join spill qualification | Local partitioned aggregate/join spill, bounded refusal, repartitioning, cleanup, and cancellation are qualified; AKS/object-store disk exhaustion and sustained cloud-scale spill remain open |
| Control-plane record sharding is not qualified against the cluster | A published-format change verified on the in-memory, filesystem and Azurite stores. The measured 11,900-owner capacity and the one-shard write cost are local figures; qualify both on AKS before either is claimed publicly |
| A commit still republishes every shard a change touches | Group commit and incremental per-shard appends are unbuilt. Writes are small enough that this is no longer urgent, and the Azurite concurrency baseline exists for when it is taken up |

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
