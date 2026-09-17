# Kaveon versus Trino — benchmark program

> How Kaveon is measured against Trino, and what a published number has to
> survive before it is a claim. Three tiers, one fairness standard, no
> single-run timings anywhere.

## The fairness standard (applies to every tier)

- **Same cluster, same nodes, one engine at a time.** `kaveon-test-aks`, one
  system node plus three worker nodes. The Trino harness scales Kaveon to zero
  while Trino runs and back, so both engines get the same three machines.
- **Same role budgets.** Coordinator 500m/1 GiB request, 2 CPU/4 GiB limit;
  workers 1 CPU/2 GiB request, 3 CPU/6 GiB limit — identical for both.
- **Same bytes.** Both engines read the same Parquet objects in ADLS. The
  runner verifies every blob's byte length and SHA-256 before a run. Kaveon
  reads the objects directly; Trino reads them through Delta or Hive external
  tables that resolve to those exact objects.
- **Exact results, checked.** Every statement carries a DuckDB reference hash;
  a wrong result is a failed execution, not a fast one.
- **Rounds, not runs.** At least five rounds, alternating engine order, five
  warm-ups per activation, medians and p95 per query, throughput as
  successful exact executions per second (the throughput tier under Tier 2's
  mechanics). Cold and warm are reported separately when the tier defines
  them.
- **Co-tenants recorded.** The runner snapshots every non-DaemonSet pod on the
  worker nodes in each engine phase and fails the run if the set changes.
- **Coverage is scored.** A query an engine cannot run is a loss for that
  engine in the summary, and listed by name. Nothing is silently dropped to
  make a table look better.
- **No production Trino.** The Trino chart exists only for this comparison; it
  is never part of a Kaveon deployment.
- **No result cache.** The coordinator keeps complete results of finished
  statements (`KAVEON_RESULT_CACHE_BYTES`, on by default). Every benchmark
  and qualification submission in this repository passes
  `settings.result_cache = false` (`scripts/scale-suite.py`,
  `scripts/differential-cases.py`, `scripts/benchmark-rounds.py` through the
  suite, `engine/qualification/aks_distributed_compare.py` and the
  qualification scripts), so a measured execution is always the Engine's.
  A record whose query records show `execution.mode = "cache"` is invalid.

## Tier 1 — matched harness on the Kaveon telemetry shapes (running)

- Fixture: `events` (5 M rows) + `customers` (100 K rows), twelve exact-result
  statements. Record: `kaveon-trino-aks-2026-09-15.md` — Kaveon 2.300 QPS vs
  Trino 1.582 (1.45×), faster on 9 of 12 shapes.
- Scale: the 504 M-row `kaveon_events_enriched` table, twenty statements with
  a Trino column. Record: `scale-suite-2026-09-16.md`.
- Time-to-answer: thirteen live statements, both engines, same object.
  Record: `kaveon-trino-time-to-answer-2026-09-15.md`.

Next on this tier: a six-round run on the dictionary-page object
(`kaveon_events_enriched_v2/combined-v2.parquet`) after the correctness sweep
image (`b8e645d`+) is rolled, with Trino re-measured on the same object.

## Tier 2 — ClickBench (the public OLAP reference)

ClickBench is the benchmark every analytical engine publishes against: one
`hits` table (99,997,497 rows, 105 columns, ~14.8 GB as Parquet) and 43
queries covering scans, filters, GROUP BY at every cardinality, string
functions, LIKE, COUNT(DISTINCT), ORDER BY … LIMIT and window-free analytics.
Trino has an official entry, which gives us two comparisons for the price of
one:

1. Kaveon vs Trino on our cluster, matched (the fairness standard above).
2. Our Trino numbers vs Trino's published ClickBench numbers, scaled by
   hardware — the check that our Trino deployment is not crippled by
   configuration. If our Trino is slower than its published self by more
   than hardware explains, the run is void until fixed.

Mechanics:

- Object: `benchmarks/clickbench/hits.parquet` in the `opensource` container,
  loaded once by a Job (`infra/aks/clickbench-load-job.yaml`) from
  `https://datasets.clickhouse.com/hits_compatible/hits.parquet`, SHA-256
  recorded in the catalog manifest.
- Queries: the upstream `queries.sql` for Trino, verbatim, one dialect fix per
  query at most, both fixes shown side by side in the record.
- Cold and warm: cold = first execution after engine activation (page cache
  dropped by the activation itself); warm = median of the next five.
- Rounds: `scripts/benchmark-rounds.py --rounds 5 --cold` alternates the two
  engines on the worker nodes five times, restarting the Engine pods before
  each Kaveon round (an empty decoded-batch cache; the object store is remote
  either way), and keeps every round's record under
  `clickbench/runs/rounds-<date>/`; `scripts/benchmark-rounds-report.py`
  reports the median over rounds of each round's median with the fastest and
  slowest round beside it. A statement that failed in any round is not
  "ran". One pass is a measurement; five rounds are a claim.
- Coverage: Kaveon does not run every ClickBench query today (URL and regexp
  functions, some casts). Every unsupported query is listed and counted as a
  loss until it runs.

### Throughput

The latency suite measures one statement at a time; the throughput tier
measures the engine under load. The metric is **successful exact executions
per second**: N concurrent clients, each running the suite's statements in a
fixed permutation seeded by its client index and looping until the window
ends; every execution is timed and its result digest checked; the figure is
successful executions divided by the measured wall seconds. Nothing else is
subtracted or weighted.

- What is counted. An execution succeeds when it returns and its digest
  (the engine-independent rendering `scripts/scale-suite.py` uses) equals the
  first digest seen for that statement on that engine in the run; a different
  digest or an error is a failure. An admission refusal — Kaveon answers
  `429 MEMORY_ADMISSION_REJECTED` (the API bridge surfaces it as HTTP 429),
  Trino `QUERY_QUEUE_FULL` — is neither: it is counted as a rejection, the
  client retries the same statement after a short backoff (0.5 s doubling to
  5 s), and the time lost counts against the engine's rate. A statement that
  fails in every attempt is listed by name in the record, as in the latency
  tiers. Per statement the record carries count, failures, rejections and
  p50/p95/max seconds; per client its counts; the warm-up is reported apart.
- Fairness. Same client count, same duration, same warm-up, same suite and
  object, one engine at a time in the same alternating windows as the latency
  suite, the client running on the system node so it is never a co-tenant of
  the workers. The same script (`scripts/benchmark-throughput.py`) drives both
  engines; only the transport differs (the API's Engine bridge for Kaveon,
  Trino's HTTP statement API for Trino). Client counts are published side by
  side (4 and 8 today), never one count for one engine against another for the
  other. The rate is reported with the failure and rejection counts beside it:
  a high rate with rejections is a scheduler refusing work, not a faster
  engine.
- How it is run. `scripts/benchmark-rounds.py --throughput 4,8` adds the tier
  to every window after the latency suite (`--throughput-duration`, default
  300 s; `--throughput-warmup`, default 30 s) and stores
  `kaveon-throughput-<clients>-round<N>.json` and `trino-…` beside the round
  records; `scripts/benchmark-rounds-report.py` then reports executions per
  second per engine and client count as the median over rounds with the
  fastest and slowest round, the summed failures and rejections, and the
  coverage list. By hand, the Kaveon side is a Job from the live API pod
  contract (`scripts/aks-scale-suite-job.py --script /input/benchmark-throughput.py
  --env ENGINE=kaveon --env CLIENTS=4 --env DURATION_SECONDS=300 --env
  WARMUP_SECONDS=30 --node-pool system`) and the Trino side is
  `infra/aks/kaveon-trino-throughput-job.yaml`, both reading the suite from
  the same ConfigMaps as the latency Jobs and ending their log with a
  `THROUGHPUT=` line. One window is a measurement; five rounds are a claim.

## Tier 3 — TPC-H SF100 (joins and subqueries)

TPC-H is the join benchmark. Twenty-two queries over eight tables with
multi-way joins, correlated subqueries, EXISTS/NOT EXISTS, and aggregates over
joins — the shapes Tier 1 and Tier 2 barely touch.

- Data: generated once with Trino's `tpch` connector at scale factor 100
  by `infra/aks/tpch-generate-job.yaml` (`scripts/generate-tpch-trino.py`,
  chart value `trino.tpch.enabled=true` for that window only) and written to
  `benchmarks/tpch/sf100/<table>/` in ADLS as Parquet through a writable
  Hive catalog; the Job's manifest (`tpch/tables.json`: columns and exact
  row counts) is what both engines register from —
  `scripts/register-tpch-catalog.py` for Kaveon (`Benchmarks.tpch_sf100`),
  the read-only `opensource` catalog for Trino.
- Queries: the standard 22 with the specification's validation parameters,
  in Trino's dialect (`tpch/trino-queries.sql`; suite `tpch/kaveon-suite.json`).
- Same rounds, same coverage rule. Kaveon's distributed joins cover
  equi-joins, broadcast builds and semi/anti joins; queries needing
  correlated subqueries or non-equi joins are listed as losses until
  implemented.

## What gets published

A tier record is one Markdown page beside its runner JSON, in this
directory, with: image digests for both engines, object byte lengths and
hashes, the co-tenant snapshot, the per-round table, per-query medians and
p95 with the ratio, the coverage list, and a section titled "What this run is
and is not". The scorecard in `docs/engineering/` quotes only records that
completed every declared round.
