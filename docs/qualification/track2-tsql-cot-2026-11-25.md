# T-SQL COT scalar qualification — 2026-11-25

## Scope

The Engine qualifies the bounded SQL Server/Fabric scalar function `COT` over
finite numeric expressions whose tangent is finite and non-zero. SQL NULL
propagates through the function. This completes the trigonometric slice
qualified on 2026-11-03, which carried `SIN`, `COS`, `TAN`, `ASIN`, `ACOS`,
`ATAN`, `DEGREES`, `RADIANS`, and `PI`; `COT` was the one member of that family
the evaluator did not implement, and a statement naming it failed with
`unknown scalar function: COT`.

SQL Server, Fabric and Trino all compute the cotangent as the reciprocal of the
tangent, so the implementation divides by `f64::tan` rather than introducing a
separate approximation. Arguments at a zero of the tangent — `COT(0)` first
among them — divide by zero and leave the bounded claim. This does not claim
SQL Server's complete numeric type, overflow, or locale behavior.

## Evidence

- Differential case: `tsql_cot_scalar_compatibility` in
  `engine/crates/server/src/differential_tests.rs`.
- The case passes the exact local/three-worker differential sweep across the
  repository's dictionary, plain, three-file, clustered, partitioned and
  Hive-ordered Parquet layouts, with the query memory pool asserted back to
  zero on the distributed path.
- Targeted sweep command:

  ```text
  KAVEON_DIFF_ONLY=tsql_cot_scalar_compatibility cargo test -p kaveon-server --bin kaveon-server differential
  ```

  Result: 221 passed, 0 failed; 49.16 seconds.

- The case was confirmed to be load-bearing rather than vacuously selected:
  with the `COT` arm removed the same command fails on
  `execution: unknown scalar function: COT`, and restoring it returns the
  sweep to green.

The argument is written `CAST(actions AS DOUBLE) / 10 + 1` so that it stays
away from zero. A case that straddled the tangent's zero would be asserting on
an infinity rather than on agreement between the layouts.

## Boundary

Arguments at a zero of the tangent, non-finite results, unsupported numeric
coercions, and external Fabric/Azure SQL execution remain outside this Engine
qualification. The authoritative versioned T-SQL corpus and its engine commit
pin are unchanged by this slice: `COT` is qualified by the differential sweep,
as the rest of the trigonometric family was, and the corpus still carries 199
cases pinned at `93d0e706`.
