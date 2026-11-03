# Bounded T-SQL numeric `FORMAT` qualification

## Contract

The Engine accepts the invariant, bounded numeric subset of SQL Server/Fabric
`FORMAT`:

- standard `N0` through `N6` patterns (grouped decimal output);
- standard `F0` through `F6` patterns (ungrouped fixed-point output); and
- fixed decimal patterns such as `0.00` and `#,##0.00`, with at most six
  fractional digits.

Integer and floating-point inputs are supported, NULLs remain NULL, and
non-finite values or unsupported .NET/culture patterns fail closed. Decimal
and culture-sensitive formatting are intentionally outside this bounded
contract; they are not silently converted through floating point.

## Evidence

- `engine/crates/exec/src/expr_eval.rs`
  `formats_numeric_values_with_bounded_invariant_patterns` covers grouped,
  fixed, negative, NULL, and rejection behavior.
- `engine/crates/sql/src/logical_plan.rs`
  `lowers_tsql_format_to_bounded_numeric_formatter` verifies the SQL Server
  spelling lowers through the normal `TO_CHAR` operator.
- `engine/crates/server/src/differential_tests.rs`
  `distributed_tsql_format_numeric_matches_local` compares local execution
  with a three-worker execution for the same statement and exact rows.

The case is recorded as case `format_numeric` in the versioned T-SQL corpus.
The qualification is bounded Engine compatibility, not general .NET format or
locale parity.
