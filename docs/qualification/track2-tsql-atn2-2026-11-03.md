# T-SQL `ATN2` qualification — 2026-11-03

## Scope

The Engine accepts SQL Server's two-argument `ATN2(y, x)` spelling and maps it
to the existing typed two-argument arctangent implementation. It preserves
numeric NULL propagation and is qualified for finite numeric scalar inputs.
The generic `ATAN2` spelling remains available for the Trino-compatible
surface; this does not claim complete SQL Server numeric overflow or coercion
parity.

## Evidence

- Implementation/test pin: `2430209c`.
- Differential case: `tsql_atn2_scalar_compatibility` in
  `engine/crates/server/src/differential_tests.rs`.
- The exact local/three-worker differential sweep passed across dictionary,
  plain, partitioned, clustered, and fragmented Parquet layouts.
- Targeted sweep command:

  ```text
  cargo test -p kaveon-server --bin kaveon-server differential_tests::the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --exact
  ```

  Result: 1 passed, 0 failed; 661.50 seconds.

The case is in the authoritative versioned T-SQL corpus with distributed
status `qualified_for_bounded_numeric_inputs`.

## Boundary

Dynamic or unsupported numeric coercions, non-finite results, and execution on
external Fabric/Azure SQL sources remain outside this Engine qualification.
