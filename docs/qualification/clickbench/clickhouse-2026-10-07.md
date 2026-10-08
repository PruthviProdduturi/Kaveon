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
## 2026-10-08 rejected lane-width increase

Doubling each local worker from eight to sixteen lanes was tested on the same
two-worker stack. Two uncached repetitions measured q35 at **27.45 s** and
**28.84 s**, and q33 at **23.81 s** and **24.57 s**. The q33 change is within
run variance and q35 regressed; the qualified default is restored to eight
lanes. No configuration change is retained from this experiment.
## 2026-10-08 rejected recent-value cache

A 4,096-slot direct-mapped cache was added temporarily to the per-batch text
arena. It preserved byte-equality checks and all columnar aggregate tests, but
two uncached q35 runs measured **27.15 s** and **27.34 s**, so it did not beat
the qualified **26.6–27.0 s** range. The code was reverted and the runtime was
rebuilt from the qualified source.
## 2026-10-08 row-group task fan-out

The distributed planner can schedule multiple scan partitions per compatible
worker through `KAVEON_SCAN_PARTITIONS_PER_WORKER`. The local ClickBench
profile sets it to four, so a two-worker deployment reads a two-file table as eight
row-group tasks instead of two whole-file tasks, allowing Parquet decode and
partial aggregation to overlap across the workers. The q35 ten-row result was
byte-for-byte identical to the qualified result; q33 preserved the same count
ordering, with only rows tied at `COUNT = 1` changing order, which SQL leaves
unspecified.

On the rebuilt two-worker Docker stack, two uncached repetitions measured q35
at **14.07 s** and **15.44 s**, and q33 at **19.79 s** and **20.99 s**. The
eight-partitions-per-worker trial measured q35 at **14.44 s** and q33 at
**22.01 s**, so it was rejected. The four-way fan-out is retained in the
ClickBench profile; ordinary deployments default to one for compatibility.
ClickHouse remains faster on this control (6.316 s q35,
4.552 s q33).

The planner now also supports a smaller aggregate exchange fan-in through
`KAVEON_EXCHANGE_PARTITIONS`. The local profile uses four merge partitions
while retaining eight source scan tasks. This reduced each source task from
eight exchange output copies to four and kept the final merge at four tasks;
the measured q35 latency remained **14.45–14.67 s**, so the dominant cost is
still source decode and string-key aggregation rather than exchange fan-in.
The two-partition trial was also unchanged at **14.54 s** and was not made the
profile default.

## 2026-10-08 rejected batch-local string coding

A narrow `GROUP BY` one-text-key plus `COUNT(*)` path was tested in the
distributed columnar partial aggregate. It reduced the resident table's
per-row probes by coding duplicate values in a temporary batch map, but the
temporary map and string copies increased memory pressure: the qualified
512 MiB query budget failed closed on q35, and a 2 GiB trial completed in
**29.2 s**, slower than the clean baseline. The implementation was removed
and the Docker runtime rebuilt from the qualified source. This confirms that
the next useful optimization must avoid materialising duplicate URL strings,
not add another hash table beside the aggregate.

## 2026-10-08 rejected positional local reader

A local `ChunkReader` using positional reads was tested to remove the seek and
file-handle work in parquet-rs' standard `File` reader. It compiled and
returned exact q33/q35 result shapes, but q35 remained **14.96 s** and q33
regressed to **29.9 s** (from the qualified ~20 s range). The reader was
removed and the portable parquet-rs path restored. The result confirms that
the remaining gap is decode and aggregation CPU, rather than local cursor
syscall overhead.

## 2026-10-08 rejected eight-worker width

A temporary six-worker expansion (eight workers total, one scan task per
worker) was run against the same two-worker baseline and file. q35 completed
in **14.42 s** and q33 in **22.06 s**, versus approximately **14–15 s** and
**20 s** on the qualified two-worker profile. The extra workers were removed:
row-group decode and exact high-cardinality aggregation, not task admission,
are the limiting stages.

## 2026-10-08 rejected native Snappy decoder

A Linux-only parquet experiment linked the native `libsnappy` decoder through a vendored parquet-rs build. The image built and the worker binary linked `libsnappy.so.1`; q35 and q33 returned the same ten-row result shape under the normal two-worker profile. It did not improve the bottleneck: q35 measured **15.34 s** and q33 **22.81 s**, versus the qualified portable path at approximately **14–15 s** and **20–21 s**. The native dependency and vendored parquet fork were removed, and the portable decoder remains the release path. The result points back to decode-to-Arrow and high-cardinality string aggregation as the next profiling target, rather than Snappy decompression alone.
## 2026-10-08 rejected sampled text fingerprint

The columnar text arena was tested with a collision-safe fingerprint over URL
prefix, middle, suffix and length instead of hashing every byte. Full equality
remained authoritative and q35 returned the exact ten rows, but q33 regressed
to **26.36 s** (qualified baseline about **20–21 s**); q35 was about **14.9 s**
with no measurable improvement. The full text fingerprint is restored. This
experiment confirms that reducing the arena's hash input does not address the
dominant decode/aggregation cost and increases probe work through collisions.

## 2026-10-08 rejected fused text COUNT path

A narrow fused operator for one UTF-8 key plus `COUNT(*)` was tested. It
combined arena admission, hash probing, group creation and count updates into
one row pass, and the execution suite remained green (**199 passed, 4
ignored**). q35 returned the exact ten rows but measured **15.03 s** on the
first clean run, versus the qualified **14–15 s** range, with no reproducible
gain. The path was removed; the general columnar aggregate remains the release
implementation until a profile identifies a larger source of CPU time.

## 2026-10-08 rejected compact single-text exchange frame

The one-text-key aggregate exchange frame was shortened from the general KP2
header to a KP3 frame whose key count and type are implicit. The frame kept
the complete UTF-8 payload and exact equality semantics, and the execution
suite remained green (**199 passed, 4 ignored**). On the rebuilt two-worker
Docker profile, q35 returned the exact ten rows in **14.729 s** and **15.740
s**, compared with the qualified **14.07–15.44 s** range. q33 completed in
**19.214 s**, within the qualified **19.79–20.99 s** range but without a
reproducible improvement. The source change was removed. Exchange framing is
therefore not the current ClickBench bottleneck; profiling should stay focused
on Parquet-to-Arrow decode and high-cardinality string aggregation.

## 2026-10-08 rejected non-null UTF-8 offset fast path

The columnar key path was specialized for non-null `StringArray` batches so it
could read Arrow offsets and the UTF-8 value buffer directly, avoiding the
per-row `value()` and null checks. The change passed the full execution suite
(**203 passed, 4 ignored**), but the clean two-worker q35 run completed in
**14.836 s**, inside the qualified **14.07–15.44 s** range. It was removed
because the end-to-end result did not demonstrate a reproducible improvement;
the remaining cost is deeper in Parquet decode and text interning.

## 2026-10-08 rejected Parquet dictionary preservation

The ClickBench file uses dictionary pages for the URL column, so an opt-in
reader was added to preserve flat UTF-8 dictionary arrays through Arrow and
feed the aggregate's dictionary-key path. It compiled cleanly, but the q35
result regressed to **26.487 s** versus the qualified **14.07–15.44 s** range.
The reader and compose switch were removed. Preserving the physical
dictionary adds more decode overhead for this file than it saves in the
aggregate, so the normal logical UTF-8 reader remains the release path.

## 2026-10-08 UTF-8 view path retained for qualification

Parquet-rs can decode flat UTF-8 columns as Arrow `Utf8View` arrays. Kaveon's
scan and columnar aggregate paths now carry that representation without
changing the logical result schema. With `KAVEON_USE_UTF8_VIEW=1` on the same
two-worker Docker profile, q35 completed in **13.896 s** and **14.256 s**, q34
in **13.938 s**, and q33 in **20.155 s**. q35 and q34 returned the exact
qualified ten-row results; q33 returned ten rows with no error. The normal
reader remains the default while this representation is qualified across the
broader SQL suite; the measured gain is currently about 5% on the URL-heavy
queries and does not close the ClickHouse gap by itself.

## 2026-10-08 retained single-probe text interning

`Arena::intern` now uses one `hashbrown::HashTable::entry` lookup for both
existing and new URL values. The previous implementation probed once for an
exact match and then performed a second insertion probe for every new value;
the replacement keeps the byte-for-byte equality check and the same arena
layout. The release execution suite passed **204 tests (4 ignored)**.

On the rebuilt two-worker Docker profile, with the result cache and statistics
disabled, q35 completed in **12.729 s, 12.961 s, and 13.035 s** across three
runs; q34 completed in **12.674 s**; q33 completed in **18.997 s**. Each
returned ten rows without an execution error. These runs improve on the prior
qualified q35 range (**14.07–15.44 s**) and q33 range (**19.79–20.99 s**),
but ClickHouse on the same mounted file remains substantially faster (q35
**6.316 s**, q33 **4.552 s**). The optimization is retained as a small,
reproducible improvement; it does not establish ClickHouse parity.

A controlled scan-partition check raised `KAVEON_SCAN_PARTITIONS_PER_WORKER`
from 4 to 8. q35 was **12.708 s** and q33 **19.076 s**, effectively the same
as the retained default, so the higher fan-out is not selected. More task
fragments alone do not remove the scan/decode bottleneck on this host.

## 2026-10-08 rejected eager text-arena reservation

Reserving one arena index slot per incoming row before URL interning was tested
to avoid hash-table growth and rehashing. It compiled and preserved the
execution suite, but the rebuilt q35 run measured **13.028 s**, within the
existing range and no better than the retained single-probe path. The eager
capacity reservation was removed; it adds memory pressure without a measured
end-to-end gain.

A one-task-per-worker run (`KAVEON_SCAN_PARTITIONS_PER_WORKER=1`) was also
qualified. q35 took **24.488 s**, versus approximately **12.8–13.0 s** with
four partitions per worker. The fewer task boundaries serialize too much
Parquet work, so the four-partition default remains selected; this does not
solve the duplicate partial-dictionary exchange.

A native release build (`RUSTFLAGS=-C target-cpu=native`) was measured on the
same two-worker host. q35 completed in **13.292 s**, with no improvement over
the generic release binary. The flag was removed so published images remain
portable across AKS and local CPUs.

## 2026-10-08 rejected integer-key final repartition

The final grouped-state merge was changed experimentally to hash-partition
all key types instead of broadcasting integer-key states to every merge
thread. q33 improved from about 19.5 s to **14.7–15.3 s**, but the result was
wrong: duplicate groups were split across merge threads, and the returned top
rows had count 1 where the qualified exact result has count 2. The change was
removed and the broadcast path restored; the restored q33 run returned the
qualified count-2 rows in **19.593 s**. This identifies a required invariant
for any future optimization: the exchange partition hash must be identical to
the final decoded group-key hash, including encoded numeric types.

A correctness-preserving variant partitioned integer grouped-state payloads before
merge and disabled the redundant per-thread selector. q33 returned the exact
qualified count-2 rows, but measured **19.207 s**, effectively unchanged from
the broadcast path at **19.593 s**. The extra partition/copy work cancels the
avoided duplicate decode, so integer-key repartition is not selected.


## 2026-10-08 rejected fixed-width exchange state and larger query memory

A fixed-size Arrow state column was tested for primitive q33/q35 aggregate
states to remove per-row Binary offsets. Mixed partial producers still emit
variable-length compact states, and Arrow correctly rejected the mixed batch
(`expected FixedSizeBinary(59) but found Binary`). The change was removed
rather than weakening the schema contract.

Raising the per-query memory ceiling from **512 MiB** to **1 GiB** was also
measured on q33. It completed correctly but regressed to **20.740 s** versus
**19.087 s** at the qualified 512 MiB setting, so the default was restored.
The current gap is therefore exchange/spill execution work, not a memory-limit
setting that can be changed without a controlled end-to-end benefit.


## 2026-10-08 rejected 16,384-row Parquet batches

The ADLS/Parquet reader batch size was raised from **8,192** to **16,384**
rows to reduce exchange batch count. Both queries remained exact and completed,
but q33 regressed to **19.726 s** (qualified 8,192-row result: **19.087 s**)
and q35 measured **13.393 s** (qualified: **12.666 s**). The larger batches
were removed and the 8,192-row default restored.


A second memory experiment raised the per-query limit to **2 GiB** and the
worker admission limit to **4 GiB**. q33 completed exactly in **19.091 s**
and q35 regressed to **17.818 s**, versus the 512 MiB qualified q33/q35
results of **19.087 s** and **12.666 s**. More admission memory does not
remove the exchange bottleneck and the defaults were restored.


An exchange fan-out reduction from **4** to **2** partitions was also tested.
The q33 run did not complete successfully: the final TopN stage hit the 512 MiB
per-query reservation ceiling (`cannot reserve 854,912 bytes` with 536,149,748
bytes already reserved). The four-partition default was restored; this confirms
that reducing fan-out increases per-task state beyond the current bounded-memory
contract.


A spill chunk cap increase from **65,536** to **131,072** groups was tested.
q33 remained exact but regressed to **20.007 s** from the 19.087 s retained
baseline. The larger chunks increase per-run memory and do not pay back their
reduced run count; the 65,536 default was restored.


An all-key partitioned final merge was tested for source-threaded grouped
aggregates, removing the broadcast selector for integer keys. q33 returned
the expected top-count rows in **19.242 s** without a material improvement; q35
failed with a bounded-memory reservation error in `fragment-project` near the
512 MiB per-worker limit. The existing text-only partitioned guard and integer
key broadcast path were restored.


The exchange chunk cutter was changed experimentally from `drain(..take)` to
`split_off` plus buffer replacement to avoid shifting the IPC tail. q35 measured
**13.036 s** and q33 **19.632 s**, both exact but slower than the retained
12.666 s / 19.087 s baseline. The original cutter was restored.


An execution-level parallelism check lowered `KAVEON_LOCAL_PARALLELISM` from
the default **8** to **4** per worker. On a freshly recreated two-worker
Docker stack, q33 remained exact but took **20.865 s**, compared with the
retained ~**19.087 s** baseline. The q35 run completed without an execution
error at about **12.8 s**, with no improvement over the retained **12.666 s**
run. The environment override was removed and the default of 8 restored.
Lowering local parallelism therefore increases contention/serialization on
this workload and is not selected.


An adaptive-partial probe was also tested. It capped only the first grouped
partial round at **16 MiB**, allowing the high-cardinality q33 shape to switch
to pass-through earlier without changing final merge semantics. q33 remained
exact at **19.166 s** and q35 at **13.434 s**, with no improvement over the
retained defaults. A **4 MiB** probe made q33 worse at **21.298 s**. The
opt-in probe and its compose setting were removed; the normal memory-sized
first round remains the release behavior.


The local Parquet reader was run on an opt-in four-thread Tokio runtime with
the object-store reader attached to that runtime. q35 remained exact at
**12.833 s** and **12.692 s** across two runs, indistinguishable from the
current-thread qualified **12.719 s** run. The opt-in runtime setting was
removed; the reader-runtime change does not address the decode cost.


A high-memory control raised the per-query limit from **512 MiB** to **4 GiB**
and worker admission from **2 GiB** to **8 GiB**. q35 remained exact but
measured **17.703 s** and **18.942 s** across two runs, slower than the
qualified 512 MiB profile. The larger live tables increase allocation and
merge cost; the 512 MiB/2 GiB defaults were restored.


A four-worker Docker scale-out was tested against the two-worker baseline on
the same host and data. Both queries remained exact, but q33 measured
**20.372 s** and q35 **14.516 s**, versus approximately **19.1 s** and
**12.7 s** with two workers. The extra workers increase exchange and storage
contention on this host rather than improving throughput, so they were
removed and the two-worker baseline restored.


An exact encoded-key selector was then tested to remove the final merge's
broadcast-and-filter step. It partitions grouped-state batches with the same
hash function used by the merge selector and disables the second filter. q33
remained exact but measured **20.376 s**; q35 failed closed at the 512 MiB
limit while materializing TopN/project state. The implementation was removed.
This confirms that repartitioning state batches still pays more copying and
memory pressure than the current broadcast path; a future win must avoid
materializing the encoded state twice rather than only change its routing.


A higher scan fan-out was tested on the 24-core host: **16** local lanes and
eight scan partitions per worker, first at the 512 MiB query cap and then at a
2 GiB cap. The 512 MiB run failed closed with per-task aggregate reservations;
the 2 GiB run completed exactly but took **17.325 s**, slower than the retained
12.7–12.9 s q35 profile. The conservative eight-lane/four-partition and
512 MiB defaults were restored. More task fan-out therefore increases
high-cardinality aggregate pressure rather than closing the ClickHouse gap.


The columnar aggregate's hash-table look-ahead was increased from **16** to
**32** rows on the same 24-core host. q35 completed exactly in **12.804 s** and
**12.827 s** with the result cache disabled, while q33 completed exactly in
**19.407 s**. This is a small improvement without changing the memory
contract, so the 32-row prefetch distance is retained for the next round.


A count-only text fast path was prototyped for dictionary and UTF-8-view
columns. It preserved the exact q35 rows but measured **13.153 s**, slower
than the retained implementation, because building a per-batch code table
added work for this nearly unique URL distribution. The prototype was removed
and the tested prefetch-only implementation remains.


The exchange `Vec::drain` path was replaced experimentally with an
offset-based chunk buffer to avoid shifting unread IPC bytes. q35 remained
within noise at **12.722 s**, while q33 regressed to **20.820 s**; the change
was removed. A single-worker, 4 GiB-memory control was also slower at
**47.698 s** for q35, confirming that the two-worker exchange is not the sole
source of the gap. The retained runtime is the two-worker, 512 MiB bounded
profile.


A temporary host-native release image (`-C target-cpu=native`) was measured
against the portable release image. q35 took **13.111 s**, slower than the
12.8 s portable result, so the compiler flag was rejected and the portable
image was rebuilt and restored.


Internal aggregation parallelism was increased from **8** to **12** lanes
without changing scan partitioning. q35 completed exactly but regressed to
**14.177 s**, so the eight-lane default was restored.

An exchange fan-out control raised `KAVEON_EXCHANGE_PARTITIONS` from **4** to **8** on the same two-worker stack. q35 stayed exact but took **14.002 s**, slower than the retained **12.8 s** profile. The four-partition default was restored; increasing destination fan-out adds exchange overhead on this host rather than improving the high-cardinality merge.

A specialized single-UTF8-key `COUNT(*)` path was tested for the q35 hot shape. It removed the arena's second deduplication probe and preserved all **200** execution tests, but the live exact q35 run measured **12.852 s**, slower than the retained **12.6–12.8 s** generic path. The path was removed and the clean generic runtime rebuilt.

An opt-in memory-mapped local Parquet reader was implemented with zero-copy range slices and passed the storage suite (**117 passed**). On the same two-worker stack, exact q35 measured **17.198 s**, materially slower than the standard reader. The mmap path, dependency, and Compose switch were removed; standard buffered file reads remain selected.

An index-growth control raised the minimum aggregate hash table from 16 to 65,536 buckets to avoid early doublings. Exact q35 measured **13.491 s** and **13.728 s**; q33 measured **18.997 s** once, with no reproducible q35 improvement and an unnecessary baseline allocation. The change was removed and the 16-bucket default restored.

A bounded-sampling URL hash (length plus first, middle, and last bytes) was tested to reduce full-string hashing. The q35 run caused the coordinator/workers to restart before producing a result, so it failed the stability gate and was removed immediately. The original hardware-CRC hash is restored.

An Arrow `StringViewArray::value_unchecked` micro-optimization was tested in the URL key loop. The exact q35 run required a retry after worker restarts and measured **13.341 s**; q33 measured **19.105 s**, with no improvement over the selected path. The change was removed and the checked access path restored.

## 2026-10-08 — StringView byte-iterator control (rejected)

The UTF-8 view grouping loop was changed temporarily to consume
`StringViewArray::bytes_iter()` and convert the already validated bytes to
`&str`, avoiding the generic `value(row)` accessor while keeping the existing
arena hash and byte-equality checks. The full execution suite remained green
(200 passed, 4 ignored). On the clean two-worker Docker profile, exact q35
completed in **12.916 s** and q33 in **19.182 s**. Those results overlap the
qualified **12.6–12.8 s / 19.0–19.5 s** ranges and do not establish a
repeatable improvement over the selected path, so the experiment was reverted
and the release source/runtime remains unchanged.

The same-host ClickHouse control was refreshed after the control rebuild on
2026-10-08: `clickhouse-local` with `FORMAT Null` completed q35 in **6.744 s**
and q33 in **5.088 s** over the same mounted `hits.parquet`. Kaveon's matching
exact runs were **12.916 s** and **19.182 s**. The challenge remains open;
these are the current comparison numbers, not a parity claim.

After reverting that experiment and rebuilding the release image, the clean
qualified Docker runtime returned exact q35 in **13.523 s** and q33 in
**20.156 s** (10 rows each, no errors). These are the authoritative Kaveon
controls for this pass; the experimental timings above are not retained as
release performance.

## 2026-10-08 — specialized packed two-key hash (rejected)

The packed group-key hash was given a length-specialized combiner for the
common two-key numeric layout, reducing the number of SplitMix rounds while
leaving equality checks and collision behavior unchanged. The execution suite
remained green (200 passed, 4 ignored). Two exact runs measured q35 at
**13.222 s** and q33 at **19.858 s**, compared with the clean controls of
**13.523 s** and **20.156 s**; the differences were within run variance and
did not beat the retained **12.6–12.8 s / 19.0–19.5 s** qualified ranges.
The branch was reverted and the generic packed hash remains selected.

## 2026-10-08 — fused COUNT/SUM/AVG update (rejected)

The columnar aggregate briefly fused q33's `COUNT(*)`, integer `SUM`, and
integer `AVG` updates into one row pass after exact slot resolution, avoiding
three accumulator scans. All **200** execution tests passed. Three exact q33
runs measured **18.530 s**, **19.981 s**, and **20.693 s**; q35 measured
**12.964 s**. The spread overlaps the clean **19.0–20.2 s / 12.8–13.5 s**
controls and does not establish a reproducible gain, so the specialization was
removed and the clean generic accumulator path restored.

## 2026-10-08 — SSE4.2 packed-key hash (rejected)

Packed numeric group keys were temporarily hashed with hardware CRC32C before
the existing avalanche, using CRC only for bucket selection. The full suite
passed (**200 passed, 4 ignored**). On the rebuilt two-worker profile, exact
q35 measured **12.822 s** and q33 **19.333 s**, which is within the clean
**12.8–13.5 s / 19.0–20.2 s** ranges and did not produce a reproducible gain.
The CRC branch was reverted; the generic packed hash remains selected.

## 2026-10-08 — release merge microbenchmarks

The existing release microbenchmarks were rerun to locate the next structural
target. The q19-shaped final merge folded **4,000,000** partial rows into
**3,750,000** groups at **9.0M rows/s** (**110–111 ns/row**) with no spill.
Under a refusing 640 MiB budget, the q33-shaped final path decoded **6M**
rows at **28 ns/row** and partitioned them at **165 ns/row**; replay and merge
took about **3.15 s** and **1.02 s** respectively while spilling. This rules
out the final merge as the dominant clean q33 bottleneck: the next material
target is scan/decode plus the worker-side high-cardinality aggregate.

## 2026-10-08 — compact integer-key decoder (rejected)

The final merge was given a validated fixed-width decoder for compact keys made
entirely of integer columns, the exact encoding used by q33. The generic
decoder remained the fallback and malformed tags, widths, null flags, and
trailing bytes still failed closed. The aggregate tests passed, but two exact
q33 runs measured **20.790 s** and **21.162 s**, and q35 measured **13.822 s**
and **14.096 s**. That did not beat the clean **19.0–20.2 s / 12.8–13.5 s**
controls, so the specialization was reverted and the portable generic decoder
remains the release path.

## 2026-10-08 — spill fan-out 32 (rejected)

Doubling `KAVEON_HASH_SPILL_PARTITIONS` from 16 to 32 was tested on the clean
two-worker image. q33 failed closed during the partial stage because the
additional fan-out could not reserve its bounded working set under the 512 MiB
query limit (**21,388,906 bytes requested with 529,264,241 already reserved**).
The default of 16 partitions was restored; no result or correctness claim is
made for this configuration.

## 2026-10-08 — local parallelism sweep

The clean two-worker image was also measured with per-query local parallelism
set to **1, 2, 4, and 8**. Exact q33 completed in **24.121 s**, **21.853 s**,
**19.643 s**, and **19.940 s**, respectively. Four threads is marginally best
on this host but does not close the ClickHouse control (**5.088 s**), so the
deployment default of eight remains unchanged for general workloads and no
parity claim is made.

## 2026-10-08 — 1 GiB query budget (rejected)

Raising the per-query budget from the qualified 512 MiB to 1 GiB did not
remove the pressure path: q33 failed closed in the parallel partial aggregate
when it needed **1,338,738** more bytes with **1,073,735,108** already
reserved. The qualified 512 MiB configuration was restored. A previously
measured 8 GiB configuration avoided spill but was slower overall, so memory
alone is not the ClickHouse gap.

## 2026-10-08 — two-integer probe specialization (rejected)

The columnar aggregate briefly used a dedicated exact probe loop for two
primitive integer keys, avoiding generic key-enum dispatch while retaining the
same hash, equality, growth, and nullable-key semantics. The full execution
suite remained green (**200 passed, 4 ignored**), but rebuilt exact controls
measured q33 at **19.962 s** and q35 at **13.639 s**, within variance and not
better than the clean qualified ranges. The specialization was reverted.

## 2026-10-08 — fixed primitive state merge (rejected)

The final merge briefly recognized q33's exact `COUNT` + integer `SUM` +
integer `AVG` compact state envelope and merged its validated fixed-width
payload without the generic state cursor. The full execution suite remained
green, but q33 measured **19.782 s**, with no reproducible gain over the clean
control. The branch was reverted; all other aggregate layouts remain on the
generic checked merge path.

An allocation-reduced variant using a fixed `(i64, i32)` uniqueness set was
also tested for q33. It remained exact but measured **21.21 s** (q35
**13.76 s**); the existing columnar partial path remains faster and the
variant was removed.

## 2026-10-08 — paired runtime controls retained

Fresh controls on the clean two-worker image confirm the current bottleneck
is the high-cardinality exchange and spill path. Raising scan partitions per
worker from 4 to 8 measured q33 **21.42 s** and was reverted. Raising the
per-query budget to 2 GiB reduced spill volume but measured q33 **19.24 s** and
q35 **15.17 s**, so it was reverted. Four local merge lanes measured q33
**19.42 s** and q35 **14.14 s**; the paired regression kept the default at
eight. Spill fan-out 8 measured q33 **19.73 s** and q35 **13.90 s** versus
fan-out 16, and was reverted. Requalifying `KAVEON_USE_UTF8_VIEW=1` measured
q33 **19.39 s** and q35 **13.75 s**, so the normal reader remains selected.
All runs returned the exact ten rows without execution errors. These controls
change no shipped defaults; they narrow the next implementation target to
reducing encoded high-cardinality exchange state and external spill work.

## 2026-10-08 — direct unique-pair partial encoder (rejected)

A narrowly gated encoder for q33's two-integer `COUNT`/`SUM`/`AVG` shape was
tested. It emitted canonical state rows directly and fell back exactly on a
duplicate or null key, but the per-row uniqueness set and key construction
cost more than the existing columnar table: q33 regressed to **21.90 s**
(q35 remained **13.57 s**). The code was removed; no result or memory
semantics changed in the shipped path.

## 2026-10-08 — raw-row aggregate exchange (rejected)

An opt-in `PartialRaw`/`FinalRaw` path preserved raw rows through the aggregate exchange and re-aggregated after repartitioning. It returned exact schemas and rows for both controls, but did not improve the paired result: q33 completed in 19.303 s and q35 in 13.775 s, versus the clean grouped-state baseline of approximately 19.7 s and 13.5 s. The q33 improvement was outweighed by the q35 regression, so the prototype was reverted and the grouped-state exchange remains the default.

## 2026-10-08 — clean default control after rebuild

The rebuilt default grouped-state path passed the worker snapshot handshake after the normal synchronization interval (`2/2` compatible workers). The paired controls completed exactly with 10 rows each: q33 `20.598 s` and q35 `13.472 s`. The q33 profile confirms the current bottleneck: near-unique grouping creates about 100 million partial groups, sends roughly 8.9 GiB of exchange payload, and each final task decodes about 2.18 GiB and spills about 2.18 GiB under the 512 MiB per-query limit. Final aggregate CPU is negligible; exchange decoding, allocation and spill dominate.

The first post-rebuild request was intentionally rejected by the coordinator while workers were still synchronizing (`NO_COMPATIBLE_WORKER`). This is expected fail-closed behavior, but deployment qualification must wait for the heartbeat to report compatible workers before issuing benchmark traffic.

Two additional clean q33 controls completed after synchronization at `19.364 s` and `20.112 s` (10 rows each), putting the observed default median at about `19.7 s`; q35 remained `13.472 s` in the paired run. This confirms normal variance around the documented baseline rather than a performance regression from the rebuild.

## 2026-10-08 — external sorted final aggregate (rejected)

An opt-in final-stage prototype sorted the exchanged grouped-state rows by the
canonical group-key bytes and merged one contiguous key at a time, allowing a
bounded-memory finalizer to spill through the existing sort operator. The
implementation compiled and the planner suite remained green, but the q33
qualification run did not reach a result after the eight scan tasks finished
(the default path normally completes in about 20 seconds); it was canceled
after the final-stage sort made no observable progress. No correctness claim
was made and the prototype was removed. The shipped grouped-state final merge
and its measured controls remain unchanged.

## 2026-10-08 — single local final-merge lane (rejected)

Reducing `KAVEON_LOCAL_PARALLELISM` from the qualified eight lanes to one was
tested to avoid duplicate final-stage readers. It remained exact but regressed
both controls: q33 **22.69 s** and q35 **25.30 s**. The default eight-lane
configuration is restored.

## 2026-10-08 — larger exchange spool read window (rejected)

The Arrow IPC spool reader was tested with a 1 MiB `BufReader` window instead of
8 KiB. Transport tests passed and both controls stayed exact, but q33 measured
**20.10 s** and q35 **13.85 s**, versus the qualified approximately 19.1 s and
13.6 s. The change was reverted; the standard reader remains selected.

## 2026-10-08 — two local final-merge lanes (rejected)

A two-lane final merge was tested to give each merge table more of the shared
512 MiB budget. It preserved exact ten-row results but regressed q33 to
**21.48 s** and q35 to **16.70 s**. Eight lanes remain the qualified default;
the added merge concurrency is preferable to the larger per-lane spill tables.

## 2026-10-08 — fixed-width primitive group-key frames

The grouped-state exchange now uses a schema-validated `KF3` frame when every
group key is a non-null primitive integer. It removes repeated per-row type and
null markers for q33 while retaining the existing `KP2` frame for nullable,
text, dictionary, and mixed keys. Columnar aggregate tests passed, q33 remained
exact with 10 rows, and the rebuilt two-worker run measured **19.15 s**. q35
also remained exact at **13.61 s**. This is a small improvement, not ClickHouse
parity; exchange fetch/decode and spill remain the dominant costs.
