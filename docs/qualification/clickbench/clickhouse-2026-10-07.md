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
| Kaveon local Docker (compact typed key frame, warmed) | two workers, four local lanes each | 38.7 s | 47.5 s | finished, exact rows |
| ClickHouse `clickhouse-local` | 2 | 6.89 s | 16.84 s | finished |
| ClickHouse `clickhouse-local` | default | 5.06 s | 10.23 s | finished |

Kaveon's earlier clean full-suite pass measured approximately 43.8 s for q33
and q35 on a warmer run. The q35 value varies materially with local cache and
CPU contention; the repeatable conclusion is the order of magnitude gap,
not the last decimal place.

The Kaveon run completed with no worker restarts, query rejects, or failures.
The ClickHouse command used `FORMAT Null` so result rendering was excluded.

## 2026-10-07 width qualification

To separate local operator cost from cluster width, the same immutable input
was run through temporary Docker workers on the same host. Every run used the
uncached statement, 8 GiB per-query memory, and exact result comparison. The
four-worker run completed q35 in **21.7 s**; eight workers reduced it to
**17.6 s**; twelve workers reduced it further to **15.5 s**. The q35 ten-row
result and counts matched ClickHouse on every run. The q33 result also matched;
its eight- and twelve-worker runs were **22.3 s** and **22.5 s**, respectively,
so that shape is currently limited by final aggregation and exchange rather
than scan width.

A 24-worker, one-lane-per-worker run reached **13.5 s** for q35, still about
1.9x slower than ClickHouse. This rules out simple worker-count scaling as the
complete fix; the operator and exchange paths need vectorized CPU work.

For a fresh local reference, ClickHouse `clickhouse-local` on the same host and
file completed q35 in **7.2 s** and q33 in **4.9 s** (default settings; the
8-thread q35 run was 7.2 s). Kaveon therefore remains behind ClickHouse by
about 2.1x on q35 and 4.6x on q33 in this run. The width result is a scaling
signal, not parity: the next qualification must profile and reduce vectorized
aggregation, Parquet decode, and final exchange CPU before claiming a win.

## 2026-10-07 follow-up: URL-key hash

After rebuilding the engine with the query-local text-key hash, the identical
`q35` request completed in **35.0 s** (35,011 ms), down from **38.2 s**
(38,226 ms) on the same local Docker stack and input. The result remained the
same ten rows and the query completed without a worker restart. This is an
8.4% improvement, not ClickHouse parity. With the conservative 512 MiB
per-query limit, the final merge still wrote about 4.5 GB of spill data. A
separate 2 GiB per-query trial reduced spill runs but took 42.2 s on this
machine, so the memory increase is not a proven optimization and is not part
of the default configuration.

The follow-up also removes q35's literal `1` from the aggregate key while
keeping it in the projected result. The warmed rerun remained about 35.1 s,
so this is not yet a wall-clock win, but partial exchange bytes fell from
about 2.27 GB to 2.14 GB per worker and the returned rows remained identical.
The change is retained because it removes work without changing semantics;
the remaining latency is still dominated by URL payload exchange and the
high-cardinality final merge.

## 2026-10-07 batch and admission qualification

The local Parquet reader now uses 65,536-row batches (the prior default was
8,192), and the query-local text hash reads aligned eight-byte words. Storage
qualification passed **117/117** tests and execution qualification passed
**199/199** tests. On the same two-worker Docker stack, with query memory set to
8 GiB and local aggregate parallelism set to 8, exact uncached runs measured
q33 at **29.2 s** and q35 at **39.2 s**; repeated runs varied with host I/O
contention. The q35 result rows and counts matched the prior exact result.

The larger admission budget removes final-stage spill, but it does not close
the ClickHouse gap: the q35 scan/partial aggregate still spends about 33 s in
the two worker tasks, while ClickHouse remains about 10.23 s with its default
thread count. This is a measured improvement path, not parity evidence.

## Change under test

The columnar aggregate hashes packed query-local key words with an inline
non-cryptographic avalanche hash. Equality is still checked on every probe, so
the result remains exact; the keyed hash remains in use for the text arena.
Primitive COUNT, SUM, AVG, and numeric MIN/MAX states are now emitted directly
in the compact wire format, without rebuilding an `AggregateState` per group.
Grouped keys now use a versioned compact typed frame: schema-known integer
widths omit repeated type and length fields, while text keeps only the length
information needed for non-final keys. The legacy frame remains readable.
The full `kaveon-exec` suite passes (199 passed, 4 ignored). The compact frame
reduced q33 by about 9% from the prior measured run and q35 by about 44% in the
warmed run; the first post-rebuild q33 run was 45.2 s, showing a cold-start
effect. URL payload copying and Arrow decode remain the dominant q35 costs.

## Next performance gate

Before claiming parity with ClickHouse, capture stage counters and a CPU
profile for q33/q35, then optimize the largest measured stage. The likely
follow-on work is a typed exchange path that avoids compact-state
encode/decode for columnar aggregates, plus scan/aggregation morsel sizing;
those are intentionally not asserted as complete by this spot comparison.

## 2026-10-07 duplicate-hash qualification

The aggregate arena previously hashed every newly admitted text key twice: once
to probe and again for the hash table insertion callback. Storing the computed
hash alongside each arena entry removes the second full-byte pass without
changing equality verification. Commit `81e9bfff` passed the full execution
suite (**199 passed, 4 ignored**) and storage remained **117 passed, 1
ignored**. On the same two-worker, 8 GiB, eight-lane Docker qualification,
uncached q35 returned the exact ClickHouse-matching rows in **32.5 s** (q35
repeats ranged 31.0–32.5 s); q33 measured **23.6 s**. This is a material
improvement over the prior 39.2 s q35 run, but ClickHouse's 10.23 s default
run is still faster by about 3.2x.

An experimental local row-group lane reader was rejected after directory-table
assembly exposed an out-of-bounds failure; it was fully reverted and is not
part of the qualified build. The next gate remains a profiled typed exchange
or scan/aggregation optimization, followed by repeated exact q33/q35 runs.

## 2026-10-07 scan-width ceiling qualification

To test whether more scan tasks alone could close q33, the same two Parquet
files were assigned to 24 temporary one-lane workers. The uncached q33 result
remained exact and completed in **23.6 s**. The stage counters explain why
width stopped helping: aggregate file-read time rose to about **32.9 s** in
aggregate, versus about **8.2 s** with two workers, because each task reopened
the same large files. This is evidence for shared decode/read reuse, not a
worker-count recommendation. The temporary workers were removed after the
run and the normal two-worker Docker stack was restored.

Two additional experiments were rejected: a fused COUNT/SUM/AVG update loop
was exact but slower (28.4–28.8 s q33), and round-robin local partial
aggregation was neutral on q33 but regressed q35 to 37.6 s. Neither is in the
qualified build.

## 2026-10-07 integer aggregate index growth

The columnar hash index now grows fourfold only for large integer-only group
keys; text-key indexes retain the previous doubling policy. This reduces
rehash passes for q33 without changing key comparison or load-factor rules.
The execution tests passed (**199 passed, 4 ignored**). Two uncached q33 runs
with 8 GiB per-query memory measured **22.0 s** and **22.3 s**, versus the
prior **25.1 s** run, and the ten returned rows were identical. The same
build's q35 runs measured **34.6–35.0 s** with identical rows, so it is a
targeted q33 improvement and not ClickHouse parity; ClickHouse remains about
4.9 s for q33 and 7.2 s for q35 on this host.

## 2026-10-08 local parallelism default

The Docker worker default local parallelism is now eight lanes (up from four)
with the normal 512 MiB per-query limit. On the clean two-worker stack, fresh
uncached runs returned the exact ten-row results and measured q35 at **30.2 s**
and q33 at **24.4 s**. This is a configuration improvement, but it remains
far from ClickHouse's **7.2 s** q35 and **4.9 s** q33 results; no parity claim
is made.

## 2026-10-08 text-key source partitioning guard

Final aggregate partials with text keys and no unsigned identifier keys may now
partition each exchange source batch once before merge workers consume it. This
avoids broadcasting the same encoded text keys to every worker. Mixed keys that
include unsigned identifiers remain on the established broadcast path because
near-unique identifiers can exhaust the normal 512 MiB query budget when
repartitioned.

The execution suite passed (**199 passed, 4 ignored**). On the rebuilt
8-lane, 512 MiB, two-worker Docker stack, q35 returned the exact ten rows in
**28.4 s** and **31.3 s** across two uncached runs; q33 returned exact results
in **25.7 s** and **25.6 s** without memory failures. These measurements are
within normal run variance and do not establish ClickHouse parity (reference
q35 ~7.2 s, q33 ~4.9 s). The change is retained as a guarded optimization,
with q33 explicitly protected from the failed mixed-key path.

## 2026-10-08 SSE4.2 text-key hashing

The columnar aggregate now uses a runtime-detected SSE4.2 CRC32C hash for
plain text group keys on x86, with the existing scalar hash as the portable
fallback. CRC only selects hash buckets; byte equality still decides key
identity, so the result semantics are unchanged.

The full execution suite passed (**199 passed, 4 ignored**). On the rebuilt
two-worker Docker stack with 8 local lanes and a 512 MiB query limit, q35
returned the exact ten rows in **27.0 s** and **27.0 s** across two uncached
runs. q33 returned exact counts and values in **37.2 s** on its first run and
**25.7 s** on its second run, with no memory failures. The q35 improvement is
repeatable against the prior ~30 s baseline, but ClickHouse remains about
**7.2 s** on q35 and **4.9 s** on q33; this is not parity.

## 2026-10-08 rejected storage and arena experiments

Two controlled changes were reverted because they failed the performance gate.
A query-local open-addressed replacement for the text arena's general-purpose
hash table preserved exact results and passed the execution suite, but q35 was
**26.6–27.0 s** and q33 regressed to **36.9–50.1 s**. Increasing the local
Parquet batch from 65,536 to 131,072 rows measured q35 at **27.55–28.03 s**.
Finally, forcing Arrow to preserve Parquet UTF-8 dictionary arrays measured
q35 at **34.7 s**. The qualified runtime is therefore unchanged: plain UTF-8
materialization, 65,536-row batches, guarded text exchange partitioning, and
SSE4.2 text hashing. These experiments narrow the next work to a profiled
decoder/aggregation design rather than another hash-table or batch-size tweak.

## 2026-10-08 storage-path control

The same 13.8 GiB ClickBench file was copied into a disposable Linux-side
Docker volume to separate Windows bind-mount overhead from Engine work. With
the qualified plain UTF-8 reader, q35 measured **21.88–22.35 s** on that
volume, versus about **26.6–27.0 s** on the Windows bind mount; q33 remained
variable (**36.6–49.3 s**) and exact. ClickHouse's reference numbers were
captured against its native host path, so the volume result is not a parity
claim until both engines use the same storage location.

An exact dictionary-code slot-reuse fast path passed the execution suite, but
preserving dictionaries on the bind mount measured q35 at **28.86 s** and was
reverted. The qualified default remains the plain reader and Windows bind
mount; the storage control identifies filesystem sharing as a measurable
benchmark confounder rather than an Engine optimization.
## 2026-10-08 same-container ClickHouse control

For a same-host control, the official `clickhouse/clickhouse-server:latest`
image read the same Windows-mounted file through `clickhouse-local` with
`max_threads = 8` and `FORMAT Null`. It completed q35 in **6.316 s** and q33
in **4.552 s**. This removes the native-host versus container-storage
ambiguity from the comparison: Kaveon's qualified bind-mount runs remain
about **27–30 s** for q35 and **24–37 s** for q33. Both controls returned
the expected result shape in non-Null validation runs; Kaveon remains behind
ClickHouse and the ClickBench goal is still open.
