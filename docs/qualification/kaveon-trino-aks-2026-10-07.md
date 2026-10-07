# KaveonDB versus Trino 483 on AKS — run d1, 2026-10-07

This is the completed six-round qualification run. The raw report is
[`kaveon-trino-aks-2026-10-07-run-d1.json`](kaveon-trino-aks-2026-10-07-run-d1.json)
and the technical gate is
[`kaveon-trino-aks-2026-10-07-claim-gate.json`](kaveon-trino-aks-2026-10-07-claim-gate.json).

## Setup

- Three-worker AKS comparison on `Standard_D4s_v3` nodes, with one engine active at a time and identical coordinator/worker resource limits.
- Kaveon and Trino images were pinned by digest. Both read the same immutable ADLS fixture: 5,000,000 `events` rows and 100,000 `customers` rows, with Parquet hashes verified before execution.
- Twelve exact-result query shapes were used: filters, grouped and high-cardinality aggregates, joins, Top-N, distinct, projections, and multi-aggregates.
- Each engine received five warmups, five measured latency repetitions per round, ten throughput repetitions at concurrency four, and six alternating rounds. Every result hash matched the declared reference.
- TLS, authentication, worker placement, co-tenant observations, and topology restoration were recorded. The Job completed with zero restoration errors and no pod restarts.

## Throughput

| Engine | Aggregate successful exact-result QPS |
|---|---:|
| Kaveon | 3.8282 |
| Trino 483 | 1.8012 |

**Observed ratio: 2.1254× Trino.** Every paired round exceeded the 1.90× target:

`2.0487×, 2.3250×, 2.0509×, 2.0923×, 2.1951×, 2.0463×`.

The technical gate passes all checks, including exact results, six complete alternating rounds, matched resources, pinned images, verified storage objects, retained Engine metrics, and restoration.

## Latency observations

Kaveon was faster on 10 of 12 median latency shapes in this run. Median milliseconds:

| Query | Kaveon | Trino |
|---|---:|---:|
| `filtered_sum` | 199.7 | 4,277.2 |
| `grouped_sum` | 306.8 | 1,199.5 |
| `join` | 438.3 | 2,114.9 |
| `small_left_join` | 433.0 | 2,318.1 |
| `topn` | 218.8 | 1,441.0 |
| `distinct` | 306.7 | 1,370.2 |
| `unfiltered_count` | 94.8 | 209.4 |
| `arithmetic_projection` | 362.0 | 300.9 |
| `medium_groups` | 321.4 | 1,493.4 |
| `high_groups` | 3,451.6 | 1,784.1 |
| `multi_aggregate` | 281.4 | 1,292.3 |
| `grouped_join` | 610.9 | 2,271.8 |

The remaining measured weakness is high-cardinality grouping, where Trino is
faster. Arithmetic projection is also slightly slower on Kaveon. The
`unfiltered_count` path is a metadata-only footer-statistics fast path, so it
has no worker execution stage; its latency and exact result are still recorded.

## Claim status

The technical evidence supports the 2.1254× throughput result on this declared
workload. The fail-closed gate still marks the broad claim as pending because
the primary metric is `proposed_pending_user_acceptance`; the comparison metric
must be explicitly accepted as “successful exact-result queries per second.”
This report does not generalize the result to all workloads or claim complete
Trino feature parity.
