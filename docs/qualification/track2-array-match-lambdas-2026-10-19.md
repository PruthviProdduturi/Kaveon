# Track 2 qualification: array match lambdas

Kaveon supports the bounded Trino higher-order predicates
`ALL_MATCH(array, element -> boolean)`, `ANY_MATCH(...)`, and
`NONE_MATCH(...)`. Lambda parameters are bound as typed element columns and
outer-row columns remain available for captured expressions.

The predicates follow Trino's three-valued rules:

- `ANY_MATCH` returns true on any true element, false when all results are
  false, and NULL when no result is true but at least one is NULL.
- `ALL_MATCH` returns false on any false element, true when all results are
  true, and NULL when no result is false but at least one is NULL.
- `NONE_MATCH` returns false on any true element, true when no result is true,
  and NULL when no result is true but at least one is NULL.

NULL arrays return NULL and empty arrays return false for `ANY_MATCH` and true
for `ALL_MATCH`/`NONE_MATCH`.

Evidence:

- `lowers_array_match_lambdas` verifies all three lambda forms in the SQL
  planner.
- `array_match_lambdas` in the distributed differential corpus compares the
  local and three-worker paths.
- The evaluator uses typed NULL element bindings and has the same result
  contract for nested comparable values.

External-client and cloud qualification remain part of the broader Track 2
boundary.
