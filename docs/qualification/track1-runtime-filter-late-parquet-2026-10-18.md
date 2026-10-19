# Late Parquet runtime filters

## Scope

Local Parquet sources now accept supported exact runtime `IN` filters after
decoding has started. They compile a mask against the projected output schema
and apply it only to subsequent batches, skipping empty batches without
replaying previously returned rows. Multiple late filters intersect. Missing,
hidden, or incompatible filter columns decline pushdown so the executor retains
its batch filter.

A running local directory scan forwards the filter to its current file and
adds the predicate to unopened files. Those files retain decoder-side filtering
and row-group pruning. The shared runtime-filter metrics count the accepted
filter once, and emitted-row metrics count rows after the late mask.

## Evidence

- A one-row-batch local file regression reads its first row, then applies two
  filters over a reordered projection. It returns exactly the remaining matching
  row, rejects NULL and nonmatching rows, and records five examined / one admitted
  future rows. Hidden and incompatible column regressions decline safely.
- A three-file directory regression reads its first row before the filter arrives,
  returns exactly `[2, 104]` afterward, and selects 3 of 14 row groups. The current
  file keeps its two previously selected row groups; unopened files select only
  one additional group. Telemetry records one filter carrying two values.
- Two independently assigned scan partitions return exactly `[2, 4, 104]` after
  each has begun reading, without duplicate or omitted matches.
- A local hash join and a three-partition `PartitionedHashJoin` over an already
  running two-file Parquet source return the same three exact pairs, preserving
  duplicate build matches and rejecting NULL keys. The partitioned path records
  one accepted filter with two distinct values, prunes unopened row groups, and
  releases query memory to zero.

From `engine/`:

```powershell
cargo test -p kaveon-storage late_runtime_filter -- --nocapture
cargo test -p kaveon-storage --lib --quiet
cargo test -p kaveon-exec runtime_filter -- --nocapture
cargo clippy -p kaveon-exec -p kaveon-storage --all-targets -- -D warnings
git diff --check
```

Results: 5 focused storage tests passed (including the existing ADLS late-start
test); the complete storage suite passed 150 tests with 1 ignored benchmark;
all 15 execution runtime-filter tests passed. Clippy and the whitespace check
passed.

## Boundary

The current file's late mask runs after decoding, so it saves downstream work
and cannot retroactively prune row groups or pages. Unopened directory files
retain predecode pruning. This is local and in-process partitioned evidence;
deployed-image behavior, cloud pruning, and cloud snapshot qualification remain
open Track 1 gates.
