# T-SQL `TRY_CAST` decimal qualification — 2026-11-04

## Boundary

Kaveon qualifies the bounded SQL Server/Fabric form
`TRY_CAST(value AS DECIMAL(p, s))` for invariant scalar expressions. Malformed
or overflowing values return a typed `NULL`; valid values use the Engine's
decimal precision/scale contract. This is the `TRY_CAST` spelling of the
already-qualified bounded decimal conversion path and does not claim locale,
money, or arbitrary-precision behavior.

The qualification covers a row-local expression that produces either numeric
text or malformed text, with `DECIMAL(10,2)`, and compares exact local and
three-worker results over the differential corpus. Null propagation and
overflow behavior remain fail-closed through the typed safe-cast evaluator.

## Evidence

- Corpus case: `try_cast_decimal` in
  `docs/qualification/tsql-read-compatibility-corpus-2026-10-28.json`.
- Differential case: `tsql_try_cast_decimal` in
  `engine/crates/server/src/differential_tests.rs`.
- Planner lowering uses the existing `TRY_CAST_DECIMAL_10_2` typed safe-cast
  path; no new execution shortcut or external-source claim is introduced.

The authoritative T-SQL corpus contains 189 cases after this qualification.
Cloud passthrough sources and general decimal
precision beyond the Engine contract remain outside this evidence.
