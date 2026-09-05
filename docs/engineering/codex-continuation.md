# Codex continuation brief

**Updated:** 2026-09-04 America/Los_Angeles  
**Branch:** `dev`  
**Repository:** `PruthviProdduturi/Kaveon`

This is the durable Engineer 2 continuation record. Read `HANDSHAKE.md` first on every machine, then this file. Never store credentials, tokens, connection strings, or user data here.

## Product boundary

For the Windows toolchain provisioned on September 4 and reproducible local
correctness gates, read [Development environment and qualification baseline](development-environment.md).
It records installed prerequisites, passing build checks, the independent
Trino/DuckDB regression harness, and five observed SQL failures. Environment
readiness does not change the Engine's alpha designation.

Kaveon is one platform with three cooperating pillars:

- Kaveon Engine: distributed vectorized SQL over customer-controlled lake storage.
- Kaveon DLM: deterministic Data Language Model that compiles governed dataset context into SQL without requiring a hosted LLM.
- Kaveon Studio: Ask, SQL Lab, dashboards, administration, and operational surfaces.

Studio Data Sources owns the user-facing connection lifecycle. The platform PostgreSQL registry and Engine native catalog are still separate authorities until the revision-aware bridge is implemented. Engine workers execute coordinator-resolved immutable source URIs and do not independently reinterpret Studio metadata.

## Required session start

```powershell
git switch dev
git pull origin dev
Get-Content HANDSHAKE.md
Get-Content docs/engineering/codex-continuation.md
git status --short
```

The working tree is shared when Claude and Codex use the same machine. Do not reset, restore, or overwrite unfamiliar changes. Claude may commit while Codex is validating; fetch and rebase immediately before pushing.

## Latest delivered state

- `22e7d8d`: reconciled SQL coverage commit after rebase. It includes Claude's SQL work and the first combined memory/ADLS foundations because those changes shared files before Claude committed.
- `62569ed`: bounded Engine cloud execution, Engine manual, Docs/About polish, Catalog Sources error-state correction, and SQL integration repairs.
- `e2ccb8a`: newer Docs header correction from Claude; deployed after `62569ed`.
- `bd41410`: made the production catalog migration independent of `pgcrypto` and ordered the additive Engine control-plane table before legacy-table reconciliation.
- `origin/dev` was `bd41410` when this brief was last verified. Always treat `origin/dev` as authoritative.

Verified on the combined tree before the final rebase:

- 218 Rust workspace tests passed with zero failures.
- `cargo clippy --workspace --all-targets -- -D warnings` passed.
- `node scripts/validate-docs.mjs` passed: 46 Markdown files, 30 routes, 8 SVGs.
- `studio/npm run type-check` passed.
- GitHub Engine workflow passed for `62569ed`.
- GitHub Deploy passed for `62569ed` and the newer `e2ccb8a`; Vercel is deployed.

Windows may warn that Cargo incremental hard links are unavailable and copy files instead. That is an environment warning, not a Rust lint failure.

## Engine runtime completed in this round

### Memory

- `MemoryAdmissionController` atomically reserves a complete query budget against a process ceiling and releases it through RAII.
- Server settings:
  - `KAVEON_QUERY_MEMORY_LIMIT_BYTES`, default 512 MiB.
  - `KAVEON_MEMORY_ADMISSION_LIMIT_BYTES`, default 4 GiB.
- Coordinator statement submission returns HTTP 429 with `MEMORY_ADMISSION_REJECTED` when the process ceiling cannot admit another query.
- Coordinator-local physical plans propagate a `QueryMemoryPool` into hash aggregate and hash join.
- Worker fragments create bounded task pools and propagate accounts into partial/single hash aggregate and hash join.
- Aggregate group/distinct state and join input/index/output growth are accounted and fail closed at the limit.
- Embedded compatibility constructors remain available without accounts, so the server is bounded but arbitrary library embeddings are not universally bounded.

### ADLS Gen2

- `AdlsParquetReader` accepts canonical `abfss://container@account.dfs.core.windows.net/path` locations.
- Authentication uses `object_store::azure::MicrosoftAzureBuilder` with environment/workload identity by default and explicit Azure CLI mode for development.
- Parquet metadata and selected ranges are read asynchronously through `ParquetObjectReader`.
- Strict projection, predicate validation, row-group pruning, deterministic `ScanPartition`, and scan metrics reuse the local Parquet contracts.
- A bounded-channel synchronous adapter implements `BatchSource` for the existing execution pipeline without unsafe code.
- Coordinator-local planning and worker fragment execution select the ADLS reader for Parquet `abfss://` sources.
- Cloud Delta-log replay is not implemented. Delta over `abfss://` fails explicitly. S3 and Iceberg remain pending.

### SQL reconciliation

- Decimal128 reference/type mismatches and strict Clippy issues were corrected.
- Local semi/anti joins use `SemiJoinOperator` for IN/NOT IN/EXISTS/NOT EXISTS planning.
- Distributed semi/anti joins remain explicitly unsupported.
- Distributed `SUM(DISTINCT)` and `AVG(DISTINCT)` fall back locally because the fragment aggregate contract does not encode those distinct states. Never remove this guard until the wire contract preserves semantics.

## Documentation and UI completed

- Engine manual: `docs/engine/`.
- Memory reference: `docs/reference/engine-memory-management.md`.
- Mirrored Studio routes under `/docs/engine/*` and `/docs/memory`.
- About Features has an explicit fixed-header scroll offset.
- Docs use the same public header family as About and a wider desktop grid.
- About logo hover background is explicitly transparent.
- Catalog Sources no longer renders the zero-source empty state when its API load failed.

## Azure and deployment state

Personal subscription:

```text
Subscription: 4ed07f02-b111-4eea-98ce-1c177d573a51
Resource group: kaveon-rg
ACR: kaveonacr.azurecr.io
Container Apps environment: kaveon-env
API: kaveon-api
PostgreSQL: kaveon-db
Migration job: none; the obsolete `kaveon-migrate` job was removed
```

The Azure CLI context can be changed by another concurrent shell. Always scope the subscription in the same command sequence before Azure operations:

```powershell
az account set --subscription 4ed07f02-b111-4eea-98ce-1c177d573a51
```

Engine ACR build `ccc2` succeeded. It built from the public Git context to avoid traversing roughly 24 GiB of local ignored Cargo targets:

```powershell
az acr task show-run --registry kaveonacr --run-id ccc2 -o table
az acr repository show-tags --name kaveonacr --repository kaveon-engine -o table
```

Published tags `62569ed` and `latest` resolve to:

```text
sha256:04bcf399d7e365934f37a026002947c01896defb4b73acb28879ebf595ab3484
```

The digest was pulled and started locally. `/health` returned HTTP 200 with Engine `0.1.0`, and `/v1/node` returned HTTP 200 with coordinator identity and measured RSS. `/ready` correctly returned not ready because the isolated smoke container had no catalog. The disposable smoke container was removed.

## Production Catalog Sources incident — closed

Azure API logs proved that `kaveonmeta` lacked `catalog_sources`. Commit `bd41410` removed the unnecessary `pgcrypto` dependency and made this additive control-plane table the first PostgreSQL schema operation, preventing unrelated legacy-table drift from blocking it. The production schema was applied through the API's managed-identity database path. An authenticated `GET /api/v1/catalog-sources` now returns HTTP 200 and `{ "success": true, "catalogSources": [] }`.

The obsolete `kaveon-migrate` Container Apps job and `kaveon-migrate` ACR repository were removed. The legacy image embedded database credentials and targeted the wrong destination database, so it must never be restored or rerun. The exposed Azure PostgreSQL administrator password was rotated and the replacement stored as `postgres-admin-password` in `kaveon-kv`; the API remained healthy because it uses managed identity. The separately embedded legacy Neon credential cannot be rotated from Azure and must be rotated or revoked in Neon before considering the incident fully contained outside Kaveon's Azure boundary. Never copy either credential into source, logs, issues, or this brief.

## Honest open gates

1. Partitioned hash-aggregate spill.
2. Partitioned/grace hash-join spill, skew handling, and broadcast thresholds.
3. Cloud Delta transaction-log/checkpoint replay over ADLS Gen2.
4. Real ADLS correctness and throughput tests using a managed/workload identity.
5. Admission queues/resource groups; current overload policy rejects with 429.
6. Streaming exchange flow control and paged/streamed root results.
7. Distributed semi/anti join and distributed SUM/AVG DISTINCT state contracts.
8. Five-worker AKS fault, concurrency, skew, memory-pressure, and comparative performance qualification.
9. Platform PostgreSQL source registry to Engine catalog synchronization bridge.
10. Replace the monolithic setup-schema replay with versioned production metadata migrations; legacy table drift still prevents a complete replay even though Catalog Sources is repaired.

Do not call Kaveon Trino-class or production-ready until the relevant gates have measured evidence. Current wording is distributed alpha.

## Next execution order

1. Rotate or revoke the exposed legacy Neon database credential in Neon.
2. Implement versioned production metadata migrations instead of replaying the monolithic setup schema.
3. Implement aggregate spill with deterministic hash partitions and recursive merge tests under tiny memory limits.
4. Implement grace hash-join spill with inner/outer correctness, null/duplicate semantics, skew limits, and cleanup tests.
5. Add ADLS integration tests against a real container, then cloud Delta log/checkpoint support.
6. Build the five-worker AKS test topology only after image and identity validation.
7. Run correctness first, then throughput/concurrency comparisons against Trino with dataset, hardware, cache state, version, and query corpus recorded.

## Standard validation

```powershell
Push-Location engine
cargo fmt --all -- --check
cargo test --workspace --no-fail-fast
cargo clippy --workspace --all-targets -- -D warnings
Pop-Location

node scripts/validate-docs.mjs
Push-Location studio
npm run type-check
Pop-Location
git diff --check
```

Before pushing:

```powershell
git pull --rebase origin dev
Get-Content HANDSHAKE.md
git push origin dev
```

Commit prefixes are `feat:`, `fix:`, `test:`, or `perf:`. Commit bodies explain why. Never add `Co-Authored-By`. No dead code, magic numbers, commented-out blocks, unsafe without documented necessity, or TODOs without issue numbers.
