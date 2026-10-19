# Track 1 — partitioned Decimal128 RANGE windows

Date: 2026-10-18

The existing Decimal128 finite `RANGE` implementation now has bounded exact
local and three-worker differential evidence. No production change was needed.

`distributed_partitioned_window_decimal_range_matches_exact_rows` covers
`DECIMAL(38,6)` and `DECIMAL(20,2)`. Each fixture has two logical partitions
interleaved over three input files, so each scan task contributes to both
partitions. One partition uses large positive and negative unscaled values
beyond floating-point integer precision; the other uses smaller values
crossing zero. Both include duplicate numeric peers and two NULL peers.

The values include exact one-unit boundaries and adjacent unscaled integers
just beyond those boundaries. This distinguishes the scaled decimal offset
from an unscaled offset and detects precision loss that would merge adjacent
high-magnitude values. Independently specified per-row counts cover ascending
and descending ordering, NULLS FIRST and LAST, and three frame shapes:

- `1 PRECEDING AND CURRENT ROW`
- `CURRENT ROW AND 1 FOLLOWING`
- `1 PRECEDING AND 1 FOLLOWING`

All 24 variants check the exact 24-row result separately through local and
in-process distributed execution. The stage graph must contain a three-way
hash exchange on `p`, and the final window stage must execute three tasks.
The query memory pool must return to zero after each execution.

Validation passed with the pinned Rust 1.98.1 toolchain:

```text
cargo test -p kaveon-server distributed_partitioned_window_decimal_range_matches_exact_rows -- --nocapture
test result: ok. 1 passed; 0 failed (24 variants, local and three-worker)

cargo test -p kaveon-exec --lib window::tests -- --nocapture
test result: ok. 16 passed; 0 failed

cargo clippy -p kaveon-server --all-targets -- -D warnings
passed

cargo fmt --all -- --check
passed

git diff --check
passed
```

This evidence covers supported nonnegative decimal scales and integer literal
offsets over Decimal128 ordering keys. Negative scales, offset overflow,
Decimal256 keys, arbitrary frame expressions, sustained pressure, and cloud
qualification remain outside this evidence. The Track 1 Windows gate remains
open.
