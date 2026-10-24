# ClickBench local Docker baseline — 2026-10-24

This is a local qualification run because AKS is unavailable until the next
cloud window. It is evidence for the Docker path only; it does not replace an
AKS qualification.

## Environment

- Host: Windows Docker Desktop, Docker Server 29.9.2
- Kaveon topology: one coordinator and two workers (`engine-worker-1` and
  `engine-worker-2`), the supported local distributed topology
- Kaveon endpoint: `http://127.0.0.1:8081/v1/trino/statement`
- Auth: local development admin principal
- Input: `data/adls-mirror/opensource/benchmarks/clickbench/hits.parquet`
  (14,779,976,446 bytes; 99,997,497 rows)
- Kaveon grouped exchange: default configuration (no override)
- ClickHouse control: `clickhouse-local`, same Parquet file, same Docker host,
  `--cpus=8`

The Kaveon and ClickHouse timings below are single completed runs after a
count smoke query. They are a baseline, not a claim of a final apples-to-
apples ranking: the Kaveon run uses two distributed workers while each
ClickHouse control is a fresh `clickhouse-local` process. Repeat with pinned
CPU/memory and warm/cold-cache phases before publishing a headline comparison.

## Results

| Query | Shape | Kaveon wall | Kaveon result | ClickHouse-local wall | Result check |
|---|---|---:|---:|---:|---|
| q33 | grouped `WatchID, ClientIP`, `COUNT/SUM/AVG`, top 10 | 24.468 s (engine 24.417 s) | 10 rows | 5.816 s | exact values inspected; parity rerun required |
| q35 | grouped constant + `URL`, `COUNT`, top 10 | 16.616 s (engine 16.567 s) | 10 rows | 8.250 s | exact values inspected; parity rerun required |

Kaveon query IDs: `c0937bb8-ab21-4d5d-9ba9-9d7b09b74c1d` (q33) and
`dd0631cd-2e47-492b-b6e1-b1fb092bfd84` (q35).

## Observed bottleneck

Both statements opened all 16 Parquet files and read 226 row groups (the
remaining row groups were pruned). q33's distributed stage 0 was 24.411 s;
its partial aggregate was effectively passthrough because the grouping keys
are near-unique, so the exchange moved large intermediate batches. q35's
distributed scan/partial stage was 16.563 s and its final merge was only
0.111 s. Admission wait and spill were zero.

The next performance work should therefore target high-cardinality grouped
aggregates: avoid needless materialization/copying for passthrough partials,
improve numeric-key hashing and scan/decode throughput, and preserve exact
results. It should be measured against this record and rerun with a pinned
ClickHouse control before any claim that Kaveon beats ClickHouse.

