# T-SQL `DATEDIFF_BIG` qualification — 2026-10-29

Status: **qualified**.

`DATEDIFF_BIG` uses the same typed calendar and fixed-unit boundary semantics
as `DATEDIFF`, with the Engine's 64-bit result representation. It is mapped at
the canonical function boundary so local and distributed execution share one
implementation and overflow policy.

## Evidence

- Canonical planner mapping: `DATEDIFF_BIG` → `DATE_DIFF`.
- Differential case: `tsql_datediff_big`, with exact local and three-worker
  results.
- Versioned corpus: 52 cases, pinned to the implementation commit.
- Formatting, focused SQL tests, corpus validation, differential sweep, strict
  Clippy, and documentation validation pass.

## Boundary

This qualifies the read expression surface; it does not claim procedural
T-SQL or SQL Server-specific overflow behavior beyond the Engine's typed
64-bit result contract.
