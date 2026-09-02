# Kaveon Product Strategy

> Ship date: **November 26, 2026 (Thanksgiving)**
> Author: Pruthvi Prodduturi — Architect & Product Owner

---

## Identity

**Tagline:** *Talk to your data.*

**Subline:** *Powered by the Data Language Model and Kaveon Engine.*

Kaveon is a unified data intelligence platform. Bring your data from any storage, query it with Kaveon Engine, explore with SQL or natural language, and build interactive dashboards — all powered by a deterministic Data Language Model.

### What Kaveon is NOT

- Not a dashboard tool (that's one surface)
- Not an LLM wrapper (DLM is deterministic, no hosted model)
- Not a connector to someone else's engine (Kaveon has its own)
- Not a copy of Superset/Metabase with better UI

### Competitive frame

Think: **Microsoft Fabric** — but open-source, with a Data Language Model instead of Copilot, and a purpose-built engine instead of renting Spark.

---

## Product Pillars

| Pillar | Identity | What it does |
|--------|----------|-------------|
| **Kaveon Engine** | The analytical database | Columnar query engine built in Rust. Current: local Parquet and vectorized execution. Target: Delta and Iceberg over ADLS Gen2/S3 through the Live Lake Path. Zero external engine dependencies. |
| **Kaveon Studio** | The intelligence layer | Dashboards (37 chart types, drag-drop canvas, cross-filtering), SQL Lab (Monaco editor), dataset management, data source registration. One surface for all analytics. |
| **Kaveon DLM** | The Data Language Model | Deterministic NL→SQL for supported question classes. Compiles per-dataset context and runs inside the shipping API; standalone API extraction is planned. Patent work is tracked privately. |

### Data model

- **Live Lake Path** (target default): Kaveon Engine reads data where it lives. Local Parquet works today; ADLS Gen2, S3, Delta Lake, and Iceberg are planned. GCS is outside the current roadmap.
- **Optimized ingest** (post-launch target): Engine reads source data and rewrites it in a sorted, partitioned, compressed layout in the same customer-controlled storage.
- **Legacy passthrough**: Current PostgreSQL / Fabric SQL / Azure SQL direct query. Migration path. Eventually deprecated.

---

## Differentiation

| What | Kaveon | Competitors |
|------|--------|-------------|
| NL→SQL | DLM — deterministic resolution for compiled, supported question classes; no model call on that path | Generative approaches can cover broader language but add model latency, cost, and nondeterminism |
| Query engine | Own engine (Rust, Parquet-native, vectorized) | Wraps DuckDB, embeds Trino, or sends SQL to external DBs |
| Architecture | Unified monorepo — Engine + Studio + DLM as one product | Glue: Superset + Trino + LangChain + 3 deploy targets |
| Data access | Live Lake Path — direct reads without mandatory import (local Parquet now; cloud target) | ETL pipelines, import jobs, data movement |
| IP | Patent in process, trademark planned, full ownership | OSS with no IP moat |

---

## Team

| Role | Who | Scope |
|------|-----|-------|
| Architect & Product Owner | Pruthvi | Vision, review, integration, patent, design decisions |
| Engineer 1 | Claude Code (+ sub-agents) | DLM API, Studio, launch prep, Engine (parallel) |
| Engineer 2 | Codex (+ sub-agents) | Engine (parallel) |

### Engine work split (by crate — zero merge conflicts)

| Crate / Issue | Codex | Claude |
|---------------|:-----:|:------:|
| `storage` — Parquet reader, ADLS Gen 2 | x | |
| `exec` — vectorized scan, hash aggregate | | x |
| `exec` — sort, TopN | x | |
| `sql` — parser, logical plan | | x |
| `optim` — filter pushdown | x | |
| `python` — PyO3 bindings | | x |
| Benchmarks vs PostgreSQL | x | |
| Integration + wiring | | x |

Total: 4-6 agents working in parallel across all pillars.

---

## Branching

| Branch | Purpose |
|--------|---------|
| `dev` | All active work. Everyone pushes here. |
| `ppe` | Pre-production. Staging and validation. |
| `main` | Public launch. Merge from `ppe` when ready. Nothing touches it until ship day. |

---

## Roadmap

### Weeks 1-4 (September 2026)

| Track | Work | Owner |
|-------|------|-------|
| Engine | Parquet reader, sort/TopN, filter pushdown | Codex |
| Engine | Hash aggregate, SQL parser | Claude |
| DLM | Standalone API extraction, OpenAPI spec, public docs | Claude |

### Weeks 5-8 (October 2026)

| Track | Work | Owner |
|-------|------|-------|
| Engine | ADLS Gen 2 reader, benchmarks vs PostgreSQL | Codex |
| Engine | PyO3 bindings, integration wiring | Claude |
| Studio | Platform rebrand — about page, landing page, data source UI ("Add Lakehouse") | Claude |

### Weeks 9-10 (November 1-15)

| Track | Work | Owner |
|-------|------|-------|
| Engine | Bug fixes, optimized hot paths | Codex |
| Studio | Polish — loading/error states, responsive pass, demo datasets | Claude |

### Weeks 11-12 (November 15-26)

| Track | Work | Owner |
|-------|------|-------|
| Engine | Final benchmarks, stability | Codex |
| Launch | README rewrite, white papers final, `docker compose up`, architecture diagrams | Claude |
| Ship | `dev` → `ppe` → `main` | All |

### **November 26: Ship.**

---

## Launch gate (all must be true)

- [ ] Engine M1 complete — `SELECT col, SUM(x) FROM parquet GROUP BY col` on local + ADLS Gen 2
- [ ] DLM works as standalone API with public documentation
- [ ] Studio reflects platform identity (not "dashboard tool")
- [ ] One-command local run with demo data — no Azure dependency for first experience
- [ ] README rewritten for public audience
- [ ] White papers finalized
- [ ] Patent provisional filed
- [ ] Architecture diagrams at Fabric-level quality
- [ ] All CI gates pass (lint, type-check, build, tests, secret scan)

## Post-launch (after Thanksgiving)

- Engine M2-M4: hash join, Delta Lake, SIMD kernels, Iceberg, parallel execution, JIT, cost-based optimizer
- Optimized ingest (OneLake-style rewrite-to-storage)
- Cloud hosting / multi-tenant (kaveon.io)
- Additional connectors: Snowflake, BigQuery, Databricks, Redshift
- Trademark filing + GitHub `kaveon` username claim
- Alerts & scheduled reports
- Dashboard embedding (guest tokens)
- Import/export dashboards

---

## Decisions locked

| Decision | Rationale |
|----------|-----------|
| DLM-led positioning | Only asset that is both unique and finished. No competitor has deterministic NL→SQL. |
| Own engine, not DuckDB/Trino | Full IP ownership. Patentable. "Powered by Kaveon Engine" is a moat. |
| Live Lake Path over import | Direct reads from customer-controlled storage without mandatory movement. |
| Optimized ingest is opt-in | Don't force a copy. Let users choose performance vs simplicity. |
| Deterministic DLM core | The DLM path requires no LLM. Optional AI-assistant features may use a separately configured hosted model. |
| Monorepo, pillars at top level | `studio/`, `api/`, `engine/`. Not nested under `apps/`. Platform-grade structure. |
| Monetization deferred | Build the best product first. Revenue model decided by traction. |
| MIT license | Open-source first. Community adoption. Re-evaluate after launch if needed. |
