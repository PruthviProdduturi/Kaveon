# Track 1 narrow integer hash-join keys — 2026-10-18

## Boundary

The hash key representation already widened `UInt32` safely, but narrow
integer columns (`Int8`, `Int16`, `UInt8`, and `UInt16`) still failed closed in
`row_key` even though the planner's type ranking treated those widths as
joinable numeric keys.

## Change

Narrow signed keys widen losslessly to the existing `Int32` hash key and narrow
unsigned keys widen losslessly to `UInt64`. The source schema remains unchanged,
so this affects hashing only; NULL keys continue to produce no match.

## Evidence

The executor regression covers all four narrow widths, NULL exclusion, signed
minimums, and unsigned maximums. The three-worker differential uses a SMALLINT
`VALUES` self-join and verifies the matching extreme row:

```text
cargo test -p kaveon-exec narrow_integer_join_keys_widen_losslessly_and_skip_nulls -- --nocapture
cargo test -p kaveon-server distributed_narrow_integer_join_keys_match_local -- --nocapture
```

Both passed locally. Wider high-cardinality and cloud join qualification remain
open.
