# T-SQL logarithm and exponential qualification — 2026-11-03

## Scope

The Engine qualifies the bounded SQL Server/Fabric numeric spellings
`LOG(value)`, `LOG(value, base)`, `LOG10(value)`, and `EXP(value)`.
Inputs are numeric expressions that produce finite values in the supported
Arrow numeric domain. SQL NULL propagates; invalid domains and non-finite
results are rejected rather than silently converted to a different type.

This is a scalar read compatibility increment. It does not claim locale,
decimal-overflow, or general SQL Server numeric-type parity.

## Evidence

- Implementation/test pin: `274b459a`.
- Differential case: `tsql_log_exp_scalar_compatibility` in
  `engine/crates/server/src/differential_tests.rs`.
- The case passed the full local/three-worker differential sweep across the
  repository's dictionary, plain, partitioned, clustered, and fragmented
  Parquet layouts.
- Targeted sweep command:

  ```text
  cargo test -p kaveon-server --bin kaveon-server differential_tests::the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --exact
  ```

  Result: 1 passed, 0 failed; 675.58 seconds.

The case is now part of the authoritative versioned T-SQL corpus and its
distributed status is `qualified_for_bounded_numeric_inputs`.

## Boundary

Dynamic bases, unsupported numeric coercions, overflow behavior outside the
Engine's finite numeric contract, and external Fabric/Azure SQL execution are
not covered. Registered external sources continue through their own
passthrough adapter.
