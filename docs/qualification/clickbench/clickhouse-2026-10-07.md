# ClickBench spot comparison — 2026-10-07

This is a local diagnostic comparison for the two high-cardinality aggregate
shapes that currently dominate Kaveon's ClickBench tail. It is not a published
cross-engine score: both engines ran on one developer machine, against the
same local `hits.parquet`, and the ClickHouse container was limited to two
threads for the constrained comparison.

## Queries

- `q33`: `GROUP BY WatchID, ClientIP` with `COUNT`, `SUM` and `AVG`, ordered by
  count and limited to ten rows.
- `q35`: `GROUP BY WatchID, URL` with `COUNT`, ordered by count and limited to
  ten rows.

## Measurements

| Engine | Threads | q33 | q35 | Result |
|---|---:|---:|---:|---|
| Kaveon local Docker (before primitive encoder) | two workers, four local lanes each | 43.9 s | 84.0 s | finished, exact rows |
| Kaveon local Docker (primitive encoder) | two workers, four local lanes each | 42.2 s | 84.8 s | finished, exact rows |
| ClickHouse `clickhouse-local` | 2 | 6.89 s | 16.84 s | finished |
| ClickHouse `clickhouse-local` | default | 5.06 s | 10.23 s | finished |

Kaveon's earlier clean full-suite pass measured approximately 43.8 s for q33
and q35 on a warmer run. The q35 value varies materially with local cache and
CPU contention; the repeatable conclusion is the order of magnitude gap,
not the last decimal place.

The Kaveon run completed with no worker restarts, query rejects, or failures.
The ClickHouse command used `FORMAT Null` so result rendering was excluded.

## Change under test

The columnar aggregate hashes packed query-local key words with an inline
non-cryptographic avalanche hash. Equality is still checked on every probe, so
the result remains exact; the keyed hash remains in use for the text arena.
Primitive COUNT, SUM, AVG, and numeric MIN/MAX states are now emitted directly
in the compact wire format, without rebuilding an `AggregateState` per group.
The full `kaveon-exec` suite passes (199 passed, 4 ignored). The primitive
encoder reduced q33 by about 4% in this run, while q35 remained unchanged;
scan, URL key encoding, Arrow decode, and/or partial/final exchange work
remain the dominant cost.

## Next performance gate

Before claiming parity with ClickHouse, capture stage counters and a CPU
profile for q33/q35, then optimize the largest measured stage. The likely
follow-on work is a typed exchange path that avoids compact-state
encode/decode for columnar aggregates, plus scan/aggregation morsel sizing;
those are intentionally not asserted as complete by this spot comparison.
