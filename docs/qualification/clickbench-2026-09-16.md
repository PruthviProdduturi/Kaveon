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
- Kaveon Engine `sha256:2600408e…` (dev `b0be459`); Trino `sha256:db58cc93…`.
- Result check: an engine-independent digest of each result set (values
  rendered canonically, rows sorted unless the statement orders them).
  Ties under `ORDER BY … LIMIT` and the epoch-versus-timestamp rendering of
  `q43` are the known legitimate differences.

## Results

| Query | Kaveon s | Trino s | Trino ÷ Kaveon | Result |
|---|---:|---:|---:|---|
| `q01` | 0.88 | 4.51 | 5.12× | same |
| `q02` | 0.72 | 4.55 | 6.34× | same |
| `q03` | 1.42 | 5.31 | 3.73× | same |
| `q04` | 2.00 | 4.11 | 2.06× | rows 1/1 |
| `q05` | 12.17 | 6.97 | 0.57× | same |
| `q06` | 19.35 | 10.79 | 0.56× | same |
| `q07` | 0.74 | 3.53 | 4.78× | rows 1/1 |
| `q08` | 0.72 | 3.55 | 4.93× | same |
| `q09` | 17.99 | 9.83 | 0.55× | same |
| `q10` | 22.90 | 19.79 | 0.86× | same |
| `q11` | 3.53 | 4.82 | 1.37× | same |
| `q12` | 7.81 | 4.98 | 0.64× | same |
| `q13` | 27.79 | 10.82 | 0.39× | same |
| `q14` | 56.59 | 17.93 | 0.32× | same |
| `q15` | 35.52 | 11.19 | 0.32× | same |
| `q16` | 41.11 | 7.38 | 0.18× | same |
| `q17` | 108.16 | 21.80 | 0.20× | same |
| `q18` | 83.53 | 19.58 | 0.23× | rows 10/10 |
| `q19` | 236.44 | 36.38 | 0.15× | same (adapted) |
| `q20` | 1.61 | 3.60 | 2.23× | same |
| `q21` | 9.30 | 12.81 | 1.38× | same |
| `q22` | 10.51 | 14.25 | 1.36× | same |
| `q23` | 16.77 | 23.96 | 1.43× | same |
| `q24` | 9.61 | 42.90 | 4.46× | rows 10/10 |
| `q25` | 3.69 | 6.21 | 1.68× | rows 10/10 |
| `q26` | 2.82 | 4.91 | 1.74× | same |
| `q27` | 3.41 | 6.01 | 1.76× | same |
| `q28` | 9.86 | 13.69 | 1.39× | rows 25/25 |
| `q29` | 189.59 | 55.33 | 0.29× | rows 25/25 |
| `q30` | 32.00 | 23.00 | 0.72× | same |
| `q31` | 28.32 | 9.33 | 0.33× | same |
| `q32` | 56.66 | 14.58 | 0.26× | rows 10/10 |
| `q33` | — Engine rejected the request | 49.75 | | |
| `q34` | 268.59 | 41.10 | 0.15× | same |
| `q35` | 309.26 | 45.39 | 0.15× | same |
| `q36` | 56.77 | 13.00 | 0.23× | same |
| `q37` | 2.24 | 3.52 | 1.57× | same |
| `q38` | 0.74 | 3.26 | 4.41× | same |
| `q39` | 0.87 | 3.49 | 4.00× | rows 10/10 |
| `q40` | 20.38 | 5.34 | 0.26× | rows 10/10 |
| `q41` | 0.59 | 3.65 | 6.15× | rows 10/10 |
| `q42` | 0.54 | 2.99 | 5.58× | rows 10/10 |
| `q43` | 0.60 | 2.91 | 4.84× | rows 10/10 (adapted) |

Both ran: 42 of 43; Kaveon faster on 22, Trino faster on 20; geometric mean of Trino ÷ Kaveon over statements both ran: 1.00×
Kaveon ran 42 of 43; Trino ran 43 of 43.

On `b0be459`, `q19`, `q33`, `q35`, `q36` were rejected on memory — the spill-capable partial aggregate the AKS workers run (`KAVEON_HASH_SPILL_ROOT`) still reserved 4 KiB per group for its state encoding — and `q43` failed on an ORDER BY over a lowered group expression. Both are fixed in `efeaeda`; its rerun of those five is folded into the table (`q19` 237 s, `q35` 309 s, `q36` 57 s, `q43` 0.6 s). `q33` (`WatchID` is unique per row: 100 M groups) still fails closed on the 3 GiB budget; on `efeaeda` the workers were briefly OOM-killed on it because a full hash table's doubling was not reserved — `39bc89f` reserves the doubling before it happens. The coordinator was OOM-killed after `q34` in the full pass; `q35`–`q43` are a rerun on the same image straight after. Run records: `clickbench/runs/kaveon-b0be459-2026-09-16.json`, `clickbench/runs/trino-3gb-2026-09-16.json`.

## What this run is and is not

- It is the first matched, same-object, same-budget ClickBench pass on the
  deployed images, with both records beside this page under
  `clickbench/runs/`.
- It is one pass per engine (three timed executions each), not the five
  alternating rounds the program requires for a published claim.
- Coverage is scored: a query an engine could not run counts as a loss for
  that engine and is listed above with its error.

## Where Kaveon loses, and why

Trino is faster on 20 of the 42 statements both engines ran, and every one of them is the same shape:

- **High-cardinality GROUP BY** (`q13`–`q19`, `q31`, `q32`, `q34`–`q36`, `q40`; `q33` fails outright): millions to a hundred million groups. Kaveon aggregates a row at a time through a general accumulator enum, with a hash probe per row on both the partial and the final stage; Trino's hash aggregation is columnar and three to five times leaner per group. On the same 3 CPUs per worker this is a 3–7× loss. The item is a columnar aggregate: typed key vectors, flat accumulator columns, vectorised hashing, and a final merge that spills the way the partial already can.
- **Exact COUNT(DISTINCT)** (`q05`, `q06`, `q09`, `q10`, `q12`, `q14`): the distinct step is single-threaded on both stages and re-hashes every row on the final; 0.5–0.9× Trino.
- **`REGEXP_REPLACE` per row** (`q29`, 0.29×) and the ninety-term `SUM(ResolutionWidth + k)` projection (`q30`, 0.72×): per-row expression evaluation with an allocation per string.

Kaveon is faster on the other 22: every scan, filter, TopN and low-cardinality aggregate (`q01`–`q04`, `q07`, `q08`, `q11`, `q20`–`q28`, `q37`–`q39`, `q41`, `q42`) by 1.4–6×, and the LIKE-heavy `q21`–`q24` by 1.4–4.5×.

## Next

- Five alternating rounds through the harness once the Engine items above
  land; cold-cache numbers; the comparison of our Trino numbers against
  Trino's published ClickBench entry scaled by hardware.
