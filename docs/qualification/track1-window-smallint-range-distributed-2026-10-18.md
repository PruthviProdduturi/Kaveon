# Track 1 — partitioned SMALLINT RANGE windows

Date: 2026-10-18

Finite `RANGE` offsets now accept Arrow `Int16` (`SMALLINT`) ordering
columns. Boundary comparison widens both values to `i128` before applying
the offset, using the existing exact integer path. This prevents wrapping
at `-32768` and `32767` and avoids conversion through floating point.

`range_offsets_support_int16_extremes_and_null_peers` covers both type
extremes, adjacent values, a gap, duplicate maximum values, and two NULL
peers. All combinations of ascending/descending order, NULLS FIRST/LAST,
and preceding/following one-unit bounds have independently specified counts.

`distributed_partitioned_window_smallint_range_matches_exact_rows` checks
16 independently specified rows in two partitions with different key
distributions. Three input files split both logical partitions across scan
tasks. For each of the same eight ordering/frame variants, both local
execution and three-worker fragment execution must match the exact counts.
The test asserts hash partitioning on `p` into three partitions and three
tasks in the final window stage. The query memory pool returns to zero
after each execution.

Validation passed with the repository's pinned Rust 1.98.1 toolchain:

```text
cargo test -p kaveon-exec --lib window::tests -- --nocapture
cargo test -p kaveon-server distributed_partitioned_window_smallint_range_matches_exact_rows -- --nocapture
cargo clippy -p kaveon-exec -p kaveon-server --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
```

The window unit suite passed all 15 tests; the focused distributed regression
passed all eight variants. Clippy, formatting, and whitespace checks passed.

This is local and in-process distributed qualification of numeric offsets
over SMALLINT keys. Temporal offsets, other unsupported key types, arbitrary
frame expressions, larger-scale workloads, and cloud qualification remain
outside this evidence. The Track 1 Windows gate remains open.
