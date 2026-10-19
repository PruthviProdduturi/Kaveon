# Join byte-skew candidate qualification — 2026-10-18

The join spill policy now finds the largest encoded-byte bucket independently
of the largest row-count bucket. Previously it checked byte imbalance only on
the bucket with the most rows, so a bucket with fewer, much wider rows could
miss the bounded secondary split.

Selection remains deterministic and bounded to one candidate per input per
spill level. A qualifying row-count candidate retains priority; otherwise the
largest byte bucket qualifies when the level contains at least 4 MiB of
encoded data and its size is at least four times the non-empty mean. The
existing requirement for at least two non-empty buckets, paired hash routing,
and maximum repartition depth remain in force. Selection reads only exact
writer-captured run metadata.

The policy regression places a 512-row wide bucket after seven 513-row narrow
buckets and requires selection of the wide bucket. A second case puts row and
byte maxima in different buckets and checks the existing row-first priority.

The execution regression generates 32 distinct keys in one primary bucket,
512 rows with independent 10,000-character payloads in that bucket, and seven
narrow buckets with 600 rows each. It forces the real 16-way spill routing
path and verifies that the byte-heavy bucket spreads across multiple child
partitions, producing exactly 31 queued partitions after one secondary split.
Inner and full outer results match the local hash join exactly, including all
payloads, duplicates, unmatched keys, and NULLs. Independent row-count checks
require 4,728 inner rows and 4,732 full outer rows. Both executions release all
query reservations and retained spill bytes.

Commands run from `engine/`:

```powershell
cargo test -p kaveon-exec skew --quiet
cargo test -p kaveon-exec --quiet
cargo clippy -p kaveon-exec --all-targets -- -D warnings
```

Results: all six skew-focused tests passed; the executor suite passed 529
tests with four intentional ignores and no failures; strict Clippy passed.
The focused tests also include the existing single-hot-key failure, bounded
depth, paired-input alignment, and cleanup cases. `git diff --check` and the
repository documentation validator passed.

This is local and disk-partitioned correctness evidence. AKS byte-skew
performance, sustained pressure, broader topology, physical disk exhaustion,
and multi-join cost policy remain open.
