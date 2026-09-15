# KaveonDB versus Trino 483 on AKS — run a3, 2026-09-15

> Matched three-worker comparison on `kaveon-test-aks` (westus2). Not a passed gate: the run completed **five of the six** declared rounds before the runner's service-account token rotated (fixed in `2747809`), and the ratio is below the declared 1.90× objective. Everything below is what was measured; nothing is extrapolated.

## Setup

- Cluster: 1 system + 3 worker nodes, Standard_D4s_v3, Kubernetes 1.35.7. One engine at a time on the worker nodes; identical role budgets (coordinator 500m/1 GiB request, 2 CPU/4 GiB limit; workers 1 CPU/2 GiB request, 3 CPU/6 GiB limit). Co-tenant topology recorded per phase and unchanged.
- Kaveon Engine `sha256:df9cfb7c…`; Trino 483 `sha256:db58cc93…`. Both read the same four ADLS objects (5,000,000-row `events`, 100,000-row `customers`, Parquet + minimal Delta log), byte lengths and SHA-256 verified before the run.
- Twelve exact-result queries; every result checked against a DuckDB reference hash on every execution. Warm cache. Five warm-ups per activation, five latency repetitions per engine per round, ten executions of every query per throughput round, concurrency four, alternating engine order per round.
- Unauthenticated statements rejected by both engines (recorded).

## Throughput — successful exact-result queries per second

| Round | Order | Kaveon | Trino |
|---|---|---:|---:|
| 1 | trino → kaveon | 2.344 | 1.738 |
| 2 | kaveon → trino | 2.297 | 1.567 |
| 3 | trino → kaveon | 2.293 | 1.564 |
| 4 | kaveon → trino | 2.368 | 1.587 |
| 5 | trino → kaveon | 2.200 | 1.456 |
| **mean of five** | | **2.300** | **1.582** |

**Ratio: 1.45× Trino** on this workload (objective 1.90×; the declared gate is not met). Round-to-round spread: Kaveon 2.200–2.368, Trino 1.456–1.738.

## Latency — median of 25 exact executions per engine (ms)

| Query | Kaveon | Trino | Trino ÷ Kaveon |
|---|---:|---:|---:|
| `filtered_sum` | 102 | 5106 | 50.25× |
| `grouped_sum` | 111 | 1438 | 13.01× |
| `join` | 1197 | 2344 | 1.96× |
| `small_left_join` | 1171 | 2593 | 2.22× |
| `topn` | 160 | 1604 | 10.03× |
| `distinct` | 481 | 1589 | 3.30× |
| `unfiltered_count` | 626 | 284 | 0.45× |
| `arithmetic_projection` | 411 | 373 | 0.91× |
| `medium_groups` | 187 | 1735 | 9.27× |
| `high_groups` | 5534 | 2127 | 0.38× |
| `multi_aggregate` | 380 | 1604 | 4.22× |
| `grouped_join` | 1209 | 2708 | 2.24× |

Kaveon is faster on 9 of 12 shapes. The three it loses are the ones that say where the Engine's cost is:

- `high_groups` — high-cardinality GROUP BY: **5.5 s against Trino's 2.1 s**. The same per-key string cost measured on the 504 M-row table earlier today (one Utf8 key multiplies a 6.5 s scan by 4–6×). This is the single largest lever left on the engine side.
- `unfiltered_count` (0.45×) and `arithmetic_projection` (0.91×): fixed per-statement overhead on a cheap query — Trino's 280–370 ms floor against Kaveon's 400–630 ms.
- Everything filter-and-aggregate shaped is 4–50× faster on Kaveon (`filtered_sum` 102 ms vs 5,106 ms), joins ~2×, TopN 10×.

## What this run is and is not

- It is a reproducible, exact-result, resource-matched comparison on the deployed images, with the full runner report beside this page (`kaveon-trino-aks-2026-09-15-run-a3.json`).
- It is not the publication gate: five rounds, not six; 1.45×, not 1.90×; warm cache only; 5 M-row tables. Earlier today's eastus lineage measured 1.26×; the runs are on different clusters and not directly comparable.
- Two runner defects found and fixed on the way (`ab6cb6d` Trino authenticator start-up gap, `2747809` token rotation); the first run of the evening aborted when the cluster's co-tenant topology changed under it, as designed.
