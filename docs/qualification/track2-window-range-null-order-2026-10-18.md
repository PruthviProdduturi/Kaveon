# Track 2 window RANGE null ordering — 2026-10-18

## Boundary

Finite `RANGE` window frames must exclude null sort values from a finite
boundary regardless of whether the window orders nulls first or last. The
executor already handled the `NULLS LAST` path, but with `NULLS FIRST` it
stopped at the leading null row and produced an empty or truncated finite
frame for later non-null rows.

## Change

`frame_bounds` now skips leading null sort values while locating a finite
`RANGE` boundary when the order specifies `NULLS FIRST`. A null current row
still resolves to its peer group, preserving SQL window semantics.

## Evidence

Focused executor coverage uses `COUNT(*) OVER (ORDER BY x NULLS FIRST RANGE
BETWEEN 1 PRECEDING AND CURRENT ROW)` over `[1, 2, NULL]` and verifies
`[1, 2, 1]` in input order:

```text
cargo test -p kaveon-exec range_offsets_with_nulls_first_skip_null_boundaries -- --nocapture
cargo test -p kaveon-exec window::tests -- --nocapture
```

Both passed locally. This qualifies the in-process window evaluator; broader
parallel window scheduling and distributed global window execution remain
separate open boundaries.
