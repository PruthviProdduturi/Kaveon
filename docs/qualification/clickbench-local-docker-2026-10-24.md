# ClickBench local Docker baseline — 2026-10-24

This is a local qualification run because AKS is unavailable until the next
cloud window. It is evidence for the Docker path only; it does not replace an
AKS qualification. The stack was rebuilt from the clean integration revision
`78b0eafd`, then from the verified executor optimization `c235c49d`, before
the final measurements.

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
| q33 | grouped `WatchID, ClientIP`, `COUNT/SUM/AVG`, top 10 | 24.223 s (engine 24.184 s) | 10 rows | 5.816 s | exact values inspected; parity rerun required |
| q35 | grouped constant + `URL`, `COUNT`, top 10 | 14.776 s (engine 14.743 s) | 10 rows | 8.250 s | exact values inspected; parity rerun required |

Kaveon query IDs: `f6e2bafe-ebac-4cb0-b651-88a21b0712e0` (q33) and
`47b099a7-1d57-43d8-8e8b-fef63eadcfb9` (q35).

## Observed bottleneck

Both statements opened all 16 Parquet files and read 226 row groups (the
remaining row groups were pruned). q33's distributed stage 0 was 24.411 s;
its partial aggregate was effectively passthrough because the grouping keys
are near-unique, so the exchange moved large intermediate batches. q35's
distributed scan/partial stage was 16.563 s and its final merge was only
0.111 s. Admission wait and spill were zero.

The fused two-key numeric aggregate path in `c235c49d` reduced q35's local
time to 14.8 s and q33's to 24.2 s, but q33 remains a passthrough aggregate
because its grouping keys are near-unique. The next performance work should
therefore target high-cardinality grouped aggregates: avoid needless
materialization/copying for passthrough partials, improve numeric-key hashing
and scan/decode throughput, and preserve exact results. It should be measured
against this record and rerun with a pinned ClickHouse control before any
claim that Kaveon beats ClickHouse.

## Exchange compression control

The same two-worker image was rerun with `KAVEON_EXCHANGE_COMPRESSION=off`.
The q33/q35 runs took 37.0 s and 18.5 s respectively, versus 24.2 s and
14.8 s with LZ4. Both runs returned the same result shape. LZ4 remains the
local default; disabling it is not an optimization for this workload.

A 12-lane-per-worker control (24 host logical cores) also did not improve the
whole workload: q33 was 25.3 s and q35 was 13.8 s versus 24.2 s and 14.8 s
at the supported eight-lane setting. The default eight-lane/two-worker
topology is retained for repeatability; more lanes alone do not remove q33's
passthrough exchange cost.

A two-partition exchange control was also slower (q33 28.0 s, q35 15.8 s),
so the four-partition default is retained.

