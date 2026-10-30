# T-SQL directional `TRIM` qualification — 2026-10-29

Engine commit `9333762e` qualifies the SQL Server character-set forms
`TRIM(LEADING characters FROM value)` and
`TRIM(TRAILING characters FROM value)`. Both lower to width-aware, vectorized
text operators, preserve NULLs, and trim Unicode scalar values contained in the
row-local character set. The existing `TRIM(BOTH ...)` path remains unchanged.

Qualification includes planner lowering and an exact local-vs-three-worker
differential query over the same event fixture. The evaluator preserves
`LargeUtf8` output width and does not allocate a second copy of the input
column. Comma-separated character lists and dynamic unsupported forms remain
fail-closed.
