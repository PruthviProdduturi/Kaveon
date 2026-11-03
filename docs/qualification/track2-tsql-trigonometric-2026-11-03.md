# T-SQL trigonometric scalar qualification — 2026-11-03

## Scope

The Engine qualifies the bounded SQL Server/Fabric scalar functions `SIN`,
`COS`, `TAN`, `ASIN`, `ACOS`, `ATAN`, `DEGREES`, `RADIANS`, and `PI` over
finite numeric expressions. SQL NULL propagates through the functions. Inputs
outside a function's mathematical domain and non-finite results are rejected;
this does not claim SQL Server's complete numeric type, overflow, or locale
behavior.

## Evidence

- Implementation/test pin: `8b6ecdef`.
- Differential case: `tsql_trigonometric_scalar_compatibility` in
  `engine/crates/server/src/differential_tests.rs`.
- The case passed the exact local/three-worker differential sweep across the
  repository's dictionary, plain, partitioned, clustered, and fragmented
  Parquet layouts.
- Targeted sweep command:

  ```text
  cargo test -p kaveon-server --bin kaveon-server differential_tests::the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --exact
  ```

  Result: 1 passed, 0 failed; 660.13 seconds.

The case is included in the authoritative versioned T-SQL corpus with
`qualified_for_bounded_numeric_inputs` distributed status.

## Boundary

Dynamic or non-finite arguments, unsupported numeric coercions, and external
Fabric/Azure SQL execution remain outside this Engine qualification. Registered
external sources continue through their own passthrough adapter.
