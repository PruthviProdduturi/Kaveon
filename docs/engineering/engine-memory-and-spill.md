# Engine memory and spill qualification

The server's memory-aware physical plans now account for hash aggregate/join,
window buffers, DISTINCT/semi-join/set-operation hash state, sort/TopN input and
merge workspaces, filter workspaces, projection output, and partial/final
aggregate-state conversion. Reservations enforce the configured pool limit and
release through RAII. Admission remains held while any worker account or
reservation still references its pool, including after a request is canceled.

## Enabling disk spill

Set `KAVEON_HASH_SPILL_ROOT` to a dedicated writable directory. Despite the
historical `HASH` prefix, this setting also enables memory-accounted external
Sort/TopN in server plans. Without it, accounted operators fail at their memory
limit. Embedded constructors without an account retain their compatibility
behavior.

| Setting | Default | Meaning |
| --- | --- | --- |
| `KAVEON_HASH_SPILL_ROOT` | Unset | Opt-in spill directory |
| `KAVEON_HASH_SPILL_BYTES` | 10 GiB | Shared disk budget for operators using one query-memory pool |
| `KAVEON_HASH_SPILL_PARTITIONS` | 16 | Fixed hash partition count, validated from 1 through 256 |
| `KAVEON_HASH_ADAPTIVE_BYTES` | Smaller of pool / 16 and 64 MiB | Accounted prefix budget before hash spill; capped at pool / 4; zero forces partitioning |

The pool's typed resource registry creates one shared spill manager. This quota
is per pool/executor, **not** a cluster-wide quota: separate worker task pools
and separate coordinator pools remain separate budgets. Use a quota-limited
volume for a process-wide disk ceiling.

Hash input is partitioned into Arrow IPC runs; a partition retains fewer than
16 run descriptors through sequential streamed compaction. Only one run reader
is open per partition source. Aggregate Single, Partial and Final modes and
inner/left/right/full/cross joins are wired into the server paths. Cross joins
and global aggregates use one partition. Equal keys never split across
partitions. Each partition must fit the memory budget, so oversized/skewed
partitions fail explicitly rather than recursively repartitioning indefinitely.

Spill-enabled aggregate and join operators first retain a bounded, accounted
prefix (at most 64 batches per input). Joins divide the byte allowance between
their inputs. If all input fits, they try the accounted in-memory operator and
avoid disk I/O on success. Only a typed query-memory admission error may replay
this prefix into spill, and only before returning any output. SQL, storage,
I/O and cancellation errors propagate without retry. The upstream source is
never reopened or reread. A batch that crosses the prefix budget is handed to
the normal partition preflight; as with other consumers, its source may already
have allocated it. Prefix reservations remain live during replay. This can
reject a tight budget conservatively; adaptive spill does not guarantee success
for every input that could fit after dropping all other buffers.

Sort/TopN use bounded-fan-in external merge. Input workspace reservations may
not be bypassed by sorting an oversized batch. Cursor batches and encoded sort
keys retain reservations; output rows are copied before advancing cursors so
they cannot retain old input buffers after their reservation is released. Merge
output batches shrink when the remaining memory cannot fit the full target.

Partial aggregate batches carry declared key types through empty and all-NULL
groups. Final merging decodes one state row at a time, reserving 32 times that row's
encoded payload plus 4 KiB for scratch. Persistent hash groups and novel distinct
values have separate reservations before insertion. Duplicate scalar states do
not retain input buffers or accumulate reservations. Grouped-state schema version 3 stores KAS/v1 compact tagged accumulators inside
the outer typed Arrow Binary column. A COUNT state is 17 bytes; per-group nested
IPC schemas are eliminated. Standalone accumulator Arrow APIs remain unchanged.
Mixed grouped-state versions fail explicitly; upgrade workers together.

## Expression expansion

String literals, concatenation, REPEAT, REPLACE, LPAD and RPAD preflight their
expanded byte size. Each is capped at 64 MiB of string output per batch. Server
expression scopes additionally charge temporary expansion to the query account
and retain those reservations for the enclosing evaluation. Overflow and budget
failure occur before allocating the expanded strings.

## Evidence and remaining limits

Focused regression coverage includes all join modes with duplicates/NULLs;
2,000 aggregate groups under 64 KiB where memory-only aggregation fails; typed
final aggregation under 4 MiB; incremental repeated scalar/distinct merging
under 256 KiB and distinct-growth rejection; skew and
disk-limit errors; early-drop file cleanup; 16 KiB sort/TopN spill; oversized
sort input rejection; DISTINCT/semi/window/set-operation budget rejection;
projection string expansion; and admission surviving cancellation with live
worker reservations.

These are **operator accounting estimates, not a process RSS guarantee**. Arrow
IPC decoding, storage decompression, allocator overhead, arbitrary expression
scratch space and transport buffers still require separate accounting and
process isolation. A source may allocate its batch before a consumer sees and
reserves it. Callers retaining emitted batches must reserve those buffers;
`RecordBatch` does not carry a memory reservation with it. Transport/result
delivery has separate quotas and validation.

Broad window frames still have quadratic CPU cost. Oversized batches, unusually
wide rows and skew can fail even with spill enabled. Normal completion, errors,
and dropping operators remove spill files through RAII; abrupt process death
can leave directories requiring operational cleanup. Production scale,
concurrency, process-RSS and worker-loss evidence remain qualification work,
not a capability inferred from the unit tests.

The native pressure harness is `engine/qualification/pressure.py`. It captures
fixture and executable SHA-256 hashes, DuckDB correctness comparisons, per-PID
RSS samples, observed spill, rejection errors and cleanup. The
`tmp/pressure-local-blocking-fixed/report.json` checkpoint passes all 11 local
cases with a 32 MiB pool for mixed operators. Active window cancellation after
297 ms recovered admission in 93 ms. Earlier cancellation and eager-exchange
prefetch failure reports are retained for comparison. Lazy exchange consumers
now decode one producer stream at a time with wire and decoded memory accounts;
local and worker CPU execution use blocking tasks so cancellation endpoints stay
responsive. These tests do not establish a hard process RSS ceiling.

The two-worker 100,000-row pressure suite with 256 MiB pools passes all ten
cases at `tmp/pressure-two-workers-compact-final/report.json` (binary SHA-256
`e5a25a1901194d12d3217251e163e51e07ce59fa34dca7572c36c5087ee57bb5`).
Unique-group aggregation returns 100,000 rows matching DuckDB, observes
4,869,136 spill bytes and cleans every spill file. Sampled worker RSS peaks are
46,219,264 and 46,272,512 bytes. The same debug pressure query took 4.547 seconds
after compact encoding and per-batch type validation, versus 12.906 seconds and
157,891,328 sampled spill bytes with incremental merging and nested IPC states
(`tmp/pressure-two-workers-incremental/report.json`). These individual debug
runs establish pressure behavior and regression direction, not matched release
performance claims. All 88 executor tests and 14 fragment execution tests pass
at this checkpoint; strict executor/server Clippy also passes.
