# The upper-hand program — Kaveon ahead of Trino on every shape

> **Status, 2026-09-17.** The tables below are the 2026-09-15 diagnosis and
> keep that day's numbers. Since then, recorded in `docs/qualification/`
> and the HANDSHAKE Log: E1 (row-group pruning on inexact string statistics,
> `95eac30`), E2 (parallel decoder lanes, `c9de306`), E4 (dictionary columns
> end to end, `13c3479`), E5 in part (typed comparisons on the lanes,
> `efc42bd`), E7/E11 (the columnar aggregate, flushing partials, parallel
> partials and DISTINCT, the hybrid final merge), D1 (the dictionary-encoded
> rebuild of the telemetry file), P3 (the coordinator result cache), G2 (the
> scale suite: 17 of 20 targets, ahead of Trino on 9 of 13,
> `scale-suite-2026-09-16.md`), G5 (ClickBench: every statement has run,
> geometric mean 1.39× over 42 on the `8d15fd3` pass,
> `clickbench-2026-09-16.md`) and G6's data (TPC-H SF100 generated as Delta,
> 21 of 22 statements plan and execute, `tpch/coverage.md`). Open: E3
> (page index and Bloom pruning), E6, E8, E9, E10, D2, D3, P1, P2, P4, P5,
> P6, G1, G3, G4, the five-round campaigns. `KAVEON_LOCAL_PARALLELISM`
> defaults to min(cores, 4) since `cf576ca`; the row below that says it
> defaults to 1 was true on 2026-09-15.

> Written 2026-09-15 from two measured runs on `kaveon-test-aks` (westus2): the matched 5 M-row fixture (Kaveon 1.45× Trino on throughput, faster on 9 of 12 shapes) and the 504 M-row time-to-answer pass (Trino faster on all 13 live corpus statements, geometric mean 5.1×). Records: `docs/qualification/kaveon-trino-aks-2026-09-15.md`, `docs/qualification/kaveon-trino-time-to-answer-2026-09-15.md`. Everything below is tied to one of those numbers; nothing is aspirational without a measurement that will prove it.

## 1. Where the seconds are

| Shape | Kaveon | Trino | Cause, from the file and the Engine's own metrics |
|---|---:|---:|---|
| Zero-row date window (`event_date` in 2025) | 10.5 s | 0.5 s | The Engine decides to prune (`row_groups_pruned: 336`) and still reads (`row_groups_read: 168`, 10.9 s wall). Trino skips every row group on footer min/max. |
| One-column full pass (`SUM(actions)`) | 6.5 s | ~2 s | Decode floor ~78 M rows/s per pass. Workers use 1.0–1.8 of 3 cores: `KAVEON_LOCAL_PARALLELISM` defaults to 1 and is unset in the chart. |
| Utf8 predicate on a clustered column (`surface = 'Chat'`) | 23.0 s | 3.8 s | Same pruning gap plus per-row string compare; the column is PLAIN-encoded in this file. |
| One Utf8 group key | 26–39 s | 10–20 s | Per-key string materialisation and hashing; strings are PLAIN, so no dictionary path for either engine. |
| Two Utf8 group keys | 82 s | 34 s | Same, ×2. |
| Fixture `high_groups` | 5.5 s | 2.1 s | High-cardinality hash aggregate — grouping cost, not I/O. |
| Fixture trivial statements | 0.4–0.6 s | 0.3 s | Per-statement floor: plan, dispatch, exchange for one row. |
| Fixture filter/aggregate/TopN/join | 0.1–1.2 s | 1.4–5.1 s | **Kaveon's win**: fused scan→filter→aggregate, bounded TopN, one exchange. Keep it. |

The pattern: Kaveon's execution model is already ahead; its Parquet reader and its use of the cores it has are behind. Both are bounded engineering, and every item below names the statement that proves it done.

## 2. Engine program (Codex — Engine crates)

Ordered by measured payoff. Each item has a done-when, taken from the statements above, on the same cluster and file.

| # | Item | What state of the art does | Done when |
|---|---|---|---|
| E1 | **Row-group pruning that actually skips I/O** | Every columnar engine (Trino, DuckDB, Velox, DataFusion) prunes on footer min/max before scheduling any byte read; a pruned row group costs nothing. | `actions in 2025` and `errors last 7 days` under 1 s; `row_groups_read` equals `row_groups_selected`. |
| E2 | **Intra-worker parallelism on scans** | Morsel-driven pipelines (DuckDB, Velox, Photon): row groups are the morsels, every core decodes one at a time, partial aggregates merge locally. The Engine already has `ParallelPartials`/`LazyFinalAggregate`; it is off by default and applies after the scan, not to it. | Worker CPU at ≥2.5 of 3 cores during a full pass; `SUM(actions)` under 3 s. |
| E3 | **Page-index and Bloom pruning** | Parquet ColumnIndex/OffsetIndex prune pages inside a row group; Bloom filters answer `=`/`IN` without decoding. Trino and DuckDB use both. | `surface = 'Chat'` and `country = 'United States'` under 4 s on a page-indexed file (see D1). |
| E4 | **Dictionary-aware execution** | Evaluate `=`/`IN` predicates once per dictionary, not per row; group on dictionary indices and map keys once per group (Velox, DuckDB, Photon). The Engine groups on dictionary keys since `e68294c`; the predicate and hash paths do not use them yet. | One Utf8 key within 1.5× of the one-column pass; two keys within 2×. |
| E5 | **Late materialisation** | Decode only the columns the filter needs, then fetch the rest for surviving rows (Velox, DuckDB `RowFilter` in arrow-rs). The reader already builds a `RowFilter`; extend it to the projection order. | Filtered aggregates read bytes proportional to selectivity (`compressed_bytes_selected`). |
| E6 | **Per-statement floor** | Plan cache keyed by SQL + catalog snapshot; single dispatch round-trip for one-fragment plans; keep-alive worker connections (already), no exchange for single-row results. | Trivial statements under 200 ms end to end. |
| E7 | **High-cardinality aggregation** | Two-level hash tables with pre-aggregation per morsel and radix partitioning before the final merge (DuckDB, ClickHouse); adaptive spill (already). | Fixture `high_groups` under 2 s. |
| E8 | **Runtime filters for joins** | Build-side Bloom filters pushed into the probe-side scan (Trino "dynamic filtering", DuckDB, Velox). | Fixture joins under 800 ms. |
| E9 | **Local SSD data cache** | Cache decoded or raw column chunks on the worker's ephemeral disk keyed by object version (Trino file-system cache, Alluxio); today's `FULL_OBJECT_CACHE` covers small objects only. | Second run of any 504 M-row statement not bound by ADLS throughput. |
| E10 | **Cost-based joins and statistics** | Column NDV/min/max from footers and from the DLM profiler, join ordering and broadcast/partition choice by size. | Fixture `grouped_join` under 800 ms; no regression on the others. |

E1 and E2 are the two-week items and together cover seven of the thirteen live corpus shapes; E3–E5 make the remaining six competitive; E6–E10 are the fixture gate.

## 3. Data layout (Claude — curation)

| # | Item | Done when |
|---|---|---|
| D1 | Rebuild the telemetry file dictionary-encoded for every string dimension, with page indexes and Bloom filters on `country`, `surface`, `platform`, `industry`; keep day-sorted row groups. Re-register with the same row count. | Both engines faster on every Utf8 shape; the Engine's dictionary path exercised. |
| D2 | Row-group sizing study: 3 M rows per group today; measure 1 M and 512 k against E2's parallelism. | Documented, one size chosen. |
| D3 | Same treatment for every OpenSource table on registration (`build-kaveon-events-parquet.py` and the curation Jobs write dictionaries + indexes by default). | Registration manifest records encodings. |

## 4. Product and DLM (Claude — `api/dlm`, Studio)

The engine will not be asked a question the DLM can answer from context; that is the product's structural advantage over a plain query engine and it widens with every item here.

| # | Item | Done when |
|---|---|---|
| P1 | Cuboid cover (`9fd2966`) live on AKS: every low-cardinality pair from context in the same scan budget. | P04/F03/F04/F08/C03/C04 from context; corpus 80/80 with those routes. |
| P2 | **Date-window precompute**: per-day and per-month breakdowns for every additive metric (one scan), so "in July", "by month", "last 7 days", "trend" are context answers; year/quarter roll up from months. | Y01–Y06, C02 from context. |
| P3 | **Exact result cache** for live statements keyed by (SQL, table version, catalog revision) with the freshness watermark the DLM already keeps. | A repeated live question answers in the cache time, not the scan time; invalidated on data change. |
| P4 | **Incremental context maintenance**: append-only tables refresh totals, breakdowns and cuboids from new row groups only (`incremental_refresh` exists for totals; extend to cuboids). | Daily refresh of the 504 M-row dataset scans one day, not the table. |
| P5 | **Adaptive precompute from usage**: the router already records what is asked; questions that went live twice become candidates for the next build's budget. | Live share of the corpus falls build over build without hand curation. |
| P6 | Time-to-answer surfaced in Studio: every answer shows its route and seconds; the dataset page shows the live share and the next build's plan. | Visible on the dataset page. |

## 5. Gates (Claude — qualification)

| # | Item | Done when |
|---|---|---|
| G1 | Six-round fixture run with the token-rotation fix; publish the ratio. | Report with six rounds. |
| G2 | **Scale suite**: the thirteen live statements as a second gate, each engine alone, medians of five, with per-statement targets from §2. | `docs/qualification` record per Engine digest; regression fails the gate. |
| G3 | Cold-cache experiment as a separate, matched run. | Report. |
| G4 | Cost per query (node-hours ÷ successful queries) recorded beside throughput. | Field in both reports. |
| G5 | **ClickBench** (Tier 2 of `docs/qualification/benchmark-program.md`): the public 100 M-row `hits` table and its 43 queries, both engines matched. First pass 2026-09-16 (`docs/qualification/clickbench-2026-09-16.md`): Kaveon ran 38 of 43, faster on 21, slower on 17, geometric mean 1.10× over the 38 — every loss is high-cardinality GROUP BY, exact COUNT(DISTINCT) or per-row REGEXP. | Kaveon runs 43 of 43 and is faster on every shape; five alternating rounds. |
| G6 | **TPC-H SF100** (Tier 3): the 22 join queries over Parquet generated once with Trino's `tpch` connector. | Record per Engine digest; coverage listed. |

The ClickBench losses name the next Engine item beyond E1–E10: **E11, a columnar aggregate** — typed key vectors, flat accumulator columns, vectorised hashing, and a final merge that spills the way the partial already can. It is what moves `q13`–`q18`, `q31`–`q36` and `q40` (0.15–0.4× Trino today); nothing else on this list does.

## 6. Sequencing and ownership

Ownership stays as in `HANDSHAKE.md`: Engine items are Codex's; data, DLM, Studio and gates are Claude's. The gates are how the two halves meet: every Engine item is accepted only when its statement passes on the cluster, and every product item is accepted only when the corpus route changes.

Week 1: E1, E2, D1, P1, P2, G2. Week 2: E3–E5, P3, G1. Weeks 3–4: E6–E10, P4–P6, G3–G4.

## 7. What "upper hand across all" means, honestly

Trino is a mature federated engine; Kaveon will not federate, and it will not out-tune Trino on a workload Trino's optimizer was built for. The claim we are building toward is narrower and stronger: **for questions people actually ask of governed tables in a lakehouse, Kaveon answers faster on every shape — most without touching the engine, the rest with a reader that uses everything the file format offers and every core it is given.** The numbers above say exactly how far that is from true today, and each row says what closes it.
