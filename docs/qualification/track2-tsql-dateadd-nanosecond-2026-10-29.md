# T-SQL `DATEADD` nanosecond qualification — 2026-10-29

Status: **qualified for nanosecond timestamp inputs**.

`DATEADD(nanosecond, amount, timestamp)` and the `ns` abbreviation use an
exact nanosecond timestamp operator. Other timestamp precisions are rejected
explicitly rather than rounded or widened with false precision.

## Evidence

- Planner lowering was covered by the versioned 51-case historical T-SQL
  corpus revision; the current authoritative corpus contains 189 cases.
- Executor regression: `date_add_nanoseconds_preserves_nanosecond_precision_and_nulls`.
- Formatting, focused SQL tests, corpus validation, strict Clippy, and
  documentation validation pass.

## Boundary

The qualified shape does not claim sub-nanosecond arithmetic or implicit
coercion from DATE, TIME, or coarser timestamp columns.
