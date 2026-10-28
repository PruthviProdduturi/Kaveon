# CLAUDE.md — Kaveon

> Repo `github.com/PruthviProdduturi/Kaveon`, folder `D:\Repos\PruthviProdduturi\Kaveon`, default branch **`dev`**, package name `kaveon`. Name lineage LoomX → Weft → Lens → **Kaveon**. This file is gitignored and syncs via the `claude-memory` repo.

## Who

Pruthvi Prodduturi — Engineering Lead & Platform Architect. Types fast with typos; parse intent, don't flag them. Terse responses — no trailing summaries. **Never add Co-Authored-By to git commits.**

## What is Kaveon

A **unified, self-hosted, open-source data intelligence platform** (MIT). Tagline *"Talk to your data."* Public demo **kaveon.io** (the old `kaveon.vercel.app` host answers 404). Ship target **Nov 26, 2026**. Competitive frame: Microsoft Fabric, but open-source, with a deterministic Data Language Model instead of Copilot and a purpose-built engine instead of rented Spark. Not a dashboard tool, not an LLM wrapper, not a wrapper over someone else's engine.

## Three pillars

| Pillar | Path | Stack | Maturity |
|---|---|---|---|
| **Kaveon Engine** | `engine/` | Rust (edition 2024), Arrow 54 / Parquet 54, sqlparser, axum, tokio | Alpha — **wired into Studio + API**: charts and SQL Lab route to `/api/v1/sql/engine` when an Engine catalog is selected (`ChartBuilderContext.tsx`, `api/routers/sql.py:execute_engine_sql`, routers `engine_catalog` / `engine_console` / `engine_governance`). No numbered Engine release yet. |
| **Kaveon DLM** | `api/dlm/` | Python — deterministic no-LLM NL→SQL + answer-from-context | Shipping, primary homepage path |
| **Kaveon Studio** | `studio/` | Next.js 15 / React 19, App Router, NextAuth v5, ECharts 5 + echarts-gl, Monaco | Live |

**Repo layout** (the old `apps/kaveon-web` + `apps/kaveon-api` split is GONE — renamed by `f7e8e27`): `engine/` (Rust workspace: crates `core storage exec sql optim python cli server`), `api/` (FastAPI, Python 3.11), `studio/` (Next.js), `packages/types`, `docs/`, `infra/bicep/`, `demo/`, `data/`, root `docker-compose.yml`.

## Co-engineering with Codex — read HANDSHAKE.md first

**`HANDSHAKE.md` is the coordination contract; read it at the start of every session.** Two engineers work this repo in parallel — Claude and **Codex** — split by ownership so there are no merge conflicts. Codex owns essentially all Engine crates (architect authorization 2026-09-03) plus, as of 2026-09-03, the API container/deploy pipeline and the open API security fixes. Claude owns `api/` (DLM, routers, services), `studio/`, launch/docs, and `engine/crates/python`. Rules: never cross an ownership boundary without recording the decision in the commit or decision log; record shipped work in the commit message and focused qualification evidence; do not append session notes to a Markdown file; use `REQUEST @Codex` / `REQUEST @Claude` to hand work across. `engine/DISTRIBUTED_EXECUTION_STATUS.md` is the Engine's durable handoff and is updated only when the reviewed capability snapshot or qualification state changes.

## Engine state

Local Parquet + local Delta (JSON commit replay from v0; **no checkpoints**), own SQL parser and logical plan, filter pushdown, projection/join pruning, Trino-style catalog (`~/.kaveon/config.toml`, SHOW/USE), remote-first `kaveon` CLI (embedded needs `--local`), HTTP statement API, coordinator + worker cluster. Distributed today: scan/filter/project, partial/final grouped and global aggregates (COUNT/SUM/MIN/MAX/weighted AVG/exact DISTINCT), Sort/TopN, repartitioned and broadcast hash joins, Arrow IPC exchange, retry/cancellation, spill-aware Sort/TopN. Implemented since that list was written — **verify before quoting any of it as a gap**: ADLS Gen2 (`adls_reader.rs`, `adls_commit.rs`), object-store Delta and Iceberg (`object_delta.rs`, `iceberg_reader.rs`), partitioned aggregate/join/Sort/TopN spill, exchange flow control (bounded replayable spool), admission queue + resource groups (`resource_groups.rs`), Engine HTTP auth (`security.rs` — bridge token, principals, Entra delegated tokens, roles) and TLS (rustls, `KAVEON_TLS_CERT_PATH`/`_KEY_PATH`). **Genuinely not done:** an S3 *reader* (S3 is only registerable as a catalog storage type), and no numbered Engine release — tags are `cli-v0.3.0` plus rolling `engine-dev`/`engine-preview`. **Implemented but not cluster-qualified:** see `engine/DISTRIBUTED_EXECUTION_STATUS.md`, which carries the remaining release gate per workstream; several read "measured locally" or "in process" and need the AKS cluster. That is the real gap, and it is a qualification gap rather than an implementation one. Gates: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, plus a clean two-worker Docker run. **Never publish single-run timings as benchmark claims.**

## DLM

Deterministic NL→SQL with no hosted model. Self-migrating tables in `kaveonmeta`: `dlm_artifact` / `dlm_value_index` / `dlm_router` / `dlm_answers` / `dlm_sketch`. Weighted router, alias + typo tolerance, injection-safe assembly. **Answer-from-context** precomputes grand totals, per-dim breakdowns and low-card 2-dim combo cells → served from an in-memory cache with no DB scan; UI shows "From context · no DB scan" vs "Live query · Xs". **HLL sketch cuboids** (p=11, pure-SQL register extraction, no `postgresql-hll` extension) cover non-additive COUNT(DISTINCT). Per-dataset context spec is curatable and survives regeneration. Patent draft at `docs/patent-adaptive-context-routing.md` — **not filed; no patent-pending labels anywhere**.

## Auth

**OAuth only** via NextAuth (Auth.js v5) in `studio/auth.ts`: GitHub, Google, Microsoft Entra ID — each activates when its env vars are set. **No local passwords / no MSAL.** Admin = email in `AUTH_ADMIN_EMAILS`, else Viewer. The browser never talks to the API; the `/api/kaveon/[...path]` proxy stamps `X-User-*` guarded by **`KAVEON_PROXY_SECRET`** (must match on studio + api). API roles `Viewer < Analyst < Editor < Admin` (`require_min_role`); content visibility `private` / `internal` / `published`.

## Deploy

Vercel (`prodduturi-labs/kaveon`) for Studio + Azure Container Apps (`kaveon-api`, ACR `kaveonacr`, RG `kaveon-rg`) + the control plane is the Engine's transaction API over object storage when `KAVEON_POSTGRESQL_RETIREMENT_MODE` is set — `docker-compose.yml` sets it for the local stack, and the hosted value lives in Azure configuration, not in this repo. Otherwise Azure PostgreSQL Flexible Server PG 18, split into **`kaveonmeta`** (control plane + DLM/context) and **`kaveon`** (data warehouse), with **Entra managed identity — there is no stored DB password**; local dev uses your `az login`. Secrets in Key Vault `kaveon-kv`. Workflows: `ci.yml` (platform + Vercel deploy), `deploy.yml` (API → ACA), `engine.yml`, `containers.yml`.

**`az acr build` runs classic Docker with NO BuildKit** — `RUN --mount=type=cache` in a Dockerfile fails there even though it works in local/Compose builds.

## Data sources

Fabric SQL, Azure SQL, PostgreSQL, MySQL, StarRocks (MySQL protocol). **Trino = planning** (no driver). Register from the UI, no `.env` edits.

## Local dev

pnpm 10, Node 22 (`.nvmrc` pins 20 but pnpm needs ≥22.13). Full stack: `docker compose up` at the repo root (postgres 17 + engine coordinator + 2 workers + api + studio; set `KAVEON_DATA_PATH`). Piecemeal: API `api/` venv + `python main.py` (:8080); Studio `next dev` (suite convention is :3002 — ARVANA owns :3000; Compose uses :3000). `.env` at repo root, read by both. npm registry is **blocked by the home eero router** → `pnpm install` fails at home; PyPI and the MS feed work.

## Preferences

No Co-Authored-By in commits — ever. · Terse, no summaries. · Parse typos by intent. · Production-grade only: no debug logging, no lowered thresholds, root-cause fixes. · Premium design bar — real SVG wordmarks, Inter only, no emoji in UI. · Professional register in all product copy. · Separate implemented / alpha / target honestly in every doc.


