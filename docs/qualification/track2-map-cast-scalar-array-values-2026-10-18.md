# Track 2: SQL MAP casts with scalar ARRAY values

The SQL parser now admits the bounded `MAP(scalar, ARRAY(scalar))` cast
shape, including Trino's `ARRAY(T)` spelling and the existing `ARRAY<T>`
spelling. The guarded fallback uses typed MAP/ARRAY AST nodes, then the
existing logical planner and recursive evaluator cast the scalar children.
This closes the SQL entry-point gap recorded in
`track2-nested-map-cast-recursive-2026-10-18.md` for this one shape.

The focused SQL tests verify the exact nested cast target for BIGINT and
DECIMAL(10, 2), reject invalid Decimal128 precision/scale, and continue to
reject complex map keys, nested MAP values, arrays of arrays/maps, and ROW
leaves. Existing scalar decimal MAP tests remain green.

`distributed_map_cast_scalar_array_values_matches_local` compares explicit
expected rows and the declared nested Arrow type against local execution
and the in-process three-worker fragment executor. Its three SQL cases cover:

- INTEGER array children widened to BIGINT, including a NULL element,
  a NULL array map value, and an empty array produced by `TRIM_ARRAY`.
- A typed NULL `MAP(VARCHAR, ARRAY(BIGINT))` across three input rows.
- DECIMAL(8, 2) children widened to DECIMAL(10, 2), preserving `1.25` and a
  NULL element.

The execution test asserts that tracked query memory returns to zero after
both local and distributed execution.

Validation from `engine/`:

```text
cargo test -p kaveon-sql map_cast -- --nocapture
cargo test -p kaveon-server distributed_map_cast_scalar_array_values_matches_local -- --nocapture
cargo test -p kaveon-server distributed_unnest_typed_null_decimal_map_preserves_schema_and_padding -- --nocapture
cargo clippy -p kaveon-sql -p kaveon-server --all-targets -- -D warnings
git diff --check
```

This is bounded local and in-process distributed evidence. It does not
qualify general recursive SQL type declarations, ROW casts, large-list
targets, Decimal256, remote workers, Trino wire clients, or AKS execution.
