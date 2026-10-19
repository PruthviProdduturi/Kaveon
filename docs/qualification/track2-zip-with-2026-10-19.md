# Track 2 qualification: `ZIP_WITH`

Kaveon supports the bounded Trino `ZIP_WITH(left, right, lambda)` shape for
two array expressions and a two-parameter lambda. Each outer row is evaluated
independently. The shorter input is padded with a typed NULL before the lambda
runs, and a NULL outer array produces a NULL result array. Lambda bodies use
the normal expression evaluator and may reference columns from the enclosing
row; the two lambda parameters shadow outer columns with the same names.

## Evidence

The parser preserves the lambda as a serializable expression:

```powershell
cargo test -p kaveon-sql lowers_zip_with_lambda_as_a_serializable_expression -- --nocapture
```

The executor applies the lambda and preserves NULL padding:

```powershell
cargo test -p kaveon-exec zip_with_applies_lambda_and_null_pads_shorter_array -- --nocapture
```

Both tests pass. Workspace Clippy also passes with `-D warnings`.

## Boundary

This qualifies two-array `ZIP_WITH` with two-parameter scalar lambdas. Lambda
parameters must be identifiers, and the lambda must return one scalar value
per pair. General higher-order array functions, additional lambda arities,
and external-client/cloud qualification remain open.
