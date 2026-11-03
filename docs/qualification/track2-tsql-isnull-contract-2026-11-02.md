# T-SQL `ISNULL` contract qualification — 2026-11-02

Kaveon preserves SQL Server `ISNULL` as a distinct bounded scalar function.
It requires exactly two arguments, returns the first argument's type, and
uses the second argument only when the first value is NULL. This is distinct
from the variadic `COALESCE` contract and prevents accidental widening or
silent acceptance of malformed T-SQL calls.

Evidence:

- Planner: `logical_plan::tests::lowers_common_tsql_scalar_aliases` retains
  `ISNULL` in the logical expression.
- Evaluator: `expr_eval::tests::evaluates_tsql_isnull_with_exact_two_argument_contract`
  verifies NULL replacement, first-value preservation, and wrong-arity
  rejection.
- Three-worker/layout differential: `KAVEON_DIFF_ONLY=tsql_isnull cargo test
  -p kaveon-server --bin kaveon-server differential_tests --quiet` — 197 tests
  passed, including dictionary, plain, multi-file, partitioned, clustered, and
  three-worker fragment paths.

This remains a scalar read-compatibility feature; SQL Server collation and
implicit code-page conversion are not claimed.
