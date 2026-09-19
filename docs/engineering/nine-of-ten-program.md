# The 9/10 program

Status: **active from 2026-09-18**. The architect's direction: every area of
the product to 9 of 10, judged as "a data team could adopt it and stay on
it", with one exclusion — the PostgreSQL retirement stays on its own track.
Scores are the honest 2026-09-18 baseline; the "9 means" column is the exit
criterion. One workstream at a time per area, each gated (fmt, clippy,
`cargo test --workspace`, the differential sweep, docs validator), merged
to `dev`, recorded in HANDSHAKE.md and `engine/DISTRIBUTED_EXECUTION_STATUS.md`.

| Area | 09-18 | 9 means | Workstreams, in order |
|---|:-:|---|---|
| Engine — knowing path | 2 | Statistics, sketches and an incremental cube answer covered questions at the pinned version; every learned answer is labelled and differentially verified. | table statistics as a versioned catalog object with the freshness signal (in flight) → clustered writes, `CLUSTER BY`, `OPTIMIZE` (in flight) → sketch-answered `APPROX_*` aggregates, opt-in, error stated → incremental cube per declared shape + `execution.mode = context` → DLM over Engine tables → learned-answer verification harness in the gate suite |
| Engine — reading path | 7 | Within 1.2× of Trino on every ClickBench statement at three nodes and the ratio holds at eight; TPC-H 22/22 measured on both engines. | adaptive partial aggregation (near-unique keys) → join spill → eight-worker run of both suites |
| Storage / connectors | 6 | Delta, Iceberg, Parquet file/directory with partition pruning and clustered layout, each qualified on ADLS; S3 qualified. | Hive partition pruning (done 09-18) → clustered writes (in flight) → Iceberg and S3 qualified on the cluster |
| DLM | 6 | Answers over lake tables with the same guarantees as over the warehouse; every answer carries its evidence; coverage is measured and published. | DLM over Engine tables with the freshness signal (`/v1/catalog/tables/{id}/version`) → evidence per answer (SQL, tables, versions, lane, reproducible) → question-class coverage report |
| Studio | 6 | A user can see why an answer is what it is, who did what, and what resources a principal may use. | evidence panel on every answer → audit and lineage views → resource-governance UI → Add table with inferred columns |
| Platform / operations | 4 | Per-principal governance, an exportable audit ledger, a versioned upgrade and rollback path, recovery from worker loss, no secret in the repo or cluster outside its store. | resource groups over the admission queue → audit ledger → engine `engine-vX.Y.Z` releases, chart pinning, catalog migrations → whole-stage retry on worker loss → secrets audit (PostgreSQL retirement excluded) |
| Evidence & trust | 7 | The site shows the campaign data; TPC-H is recorded; the ClickBench listing is submitted; anyone can rerun the suites from the repo. | five-round data on the page → TPC-H record → reproduction script → ClickBench PR |
| Docs | 7 | No duplicated topic; the connectors, settings and runbook pages are validated by CI against code; the learning engine has one reference page and a decision log. | **`docs/engine/learning-engine.md`** (ANALYZE's three forms — metadata / `sketches = true` / `distinct` — what each reads and produces; the statistics object and its source version; what answers from statistics vs. reads; the `context` mode on the record; `use_statistics` and `approximate`; sketch error bounds; file, page and Bloom skipping; clustered layout and `OPTIMIZE`; partition columns; the freshness endpoints; the differential guarantees) → **`docs/engineering/decision-log.md`** (dated non-obvious calls with their reasons) → the four consolidations from the 09-17 audit → CI checks for settings/connectors tables against `config.rs` and the catalog formats |

Rules carried from the benchmark program: never publish a single-run
number; Trino is the internal yardstick only; no debug logging, no lowered
thresholds, root causes only; one commit per change with tests; never
add attribution trailers; versioned releases only.
