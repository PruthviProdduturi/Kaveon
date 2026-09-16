# ClickBench — KaveonDB versus Trino 483 on AKS, 2026-09-16

> Tier 2 of `benchmark-program.md`: the public ClickBench `hits` table
> (99,997,497 rows, 105 columns, one 14.78 GB Parquet object) and the 43
> upstream queries, both engines alone on the same three worker nodes of
> `kaveon-test-aks`, reading the same object from ADLS. First pass; not a
> gate, not a claim. The numbers below are medians of three timed executions
> after one warm-up, per statement, per engine.

## Setup

- Cluster: 1 system + 3 worker nodes, Standard_D4s_v3 (4 vCPU, 16 GiB).
  Role budgets identical for both engines: coordinator 500m/1 GiB request,
  2 CPU/4 GiB limit; workers 1 CPU/2 GiB request, 3 CPU/6 GiB limit.
- Memory per query per worker: **3 GiB on both** — Kaveon
  `KAVEON_QUERY_MEMORY_LIMIT_BYTES=3221225472` (admission 4 GiB), Trino
  `query.max-memory-per-node=3GB` with a 5G worker heap (the first Trino pass
  ran at the chart's earlier 512 MB and is kept beside this one as
  `clickbench/runs/trino-512mb-2026-09-16.json`; eight of its queries failed
  on memory).
- Object: `opensource/benchmarks/clickbench/hits.parquet`, 14,779,976,446
  bytes, SHA-256 `a390f6cb782f6aaef278c72fc1dd86c4f30bc843ebab3c159e9bd4d45ddb079f`,
  loaded by `infra/aks/clickbench-load-job.yaml`. Kaveon reads it as
  `Benchmarks.clickbench.hits`; Trino through a Hive external table over the
  same directory (`clickbench/trino-tables.json`).
- Queries: `clickbench/trino-queries.sql`, upstream text verbatim. Two need
  a dialect adaptation because `EventTime` is stored as epoch seconds
  (upstream Trino wraps the table in a `from_unixtime` view): `q19` and
  `q43`, both shown side by side in `clickbench/kaveon-suite.json`.
- Kaveon Engine: `KAVEON_ENGINE_DIGEST`; Trino `sha256:db58cc93…`.
- Result check: an engine-independent digest of each result set (values
  rendered canonically, rows sorted unless the statement orders them).
  Ties under `ORDER BY … LIMIT` and the epoch-versus-timestamp rendering of
  `q43` are the known legitimate differences.

## Results

RESULTS_TABLE

## What this run is and is not

- It is the first matched, same-object, same-budget ClickBench pass on the
  deployed images, with both records beside this page under
  `clickbench/runs/`.
- It is one pass per engine (three timed executions each), not the five
  alternating rounds the program requires for a published claim.
- Coverage is scored: a query an engine could not run counts as a loss for
  that engine and is listed above with its error.

## Where Kaveon loses, and why

LOSSES

## Next

- Five alternating rounds through the harness once the Engine items above
  land; cold-cache numbers; the comparison of our Trino numbers against
  Trino's published ClickBench entry scaled by hardware.
