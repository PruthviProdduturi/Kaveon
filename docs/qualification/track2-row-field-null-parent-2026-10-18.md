# Track 2 SQL compatibility qualification — nullable ROW dereference

**Scope:** `ROW_FIELD` dereference of a nullable Arrow `STRUCT`/Trino `ROW`.

When a row value is NULL, dereferencing one of its fields must return NULL,
even if the physical child array contains a value in that slot. Kaveon's
evaluator now applies the parent row validity bitmap to the selected child
column. Non-null rows continue to return the child without an extra copy.

Projection planning also resolves the selected child type from the input
STRUCT schema instead of probing the evaluator with an empty batch. This keeps
the output schema identical when a distributed fragment has no rows, and
prevents a valid ROW field from being published as Arrow `Null`.

## Validation

From `engine/`:

```powershell
cargo fmt --all -- --check
cargo test -p kaveon-exec row_field --lib --quiet
cargo clippy -p kaveon-exec --lib -- -D warnings
```

The focused test suite passes both the existing named-field dereference case
and a nullable-parent case whose results are `[7, NULL]`. The server
differential test `distributed_row_subscript_matches_local_and_rejects_invalid_constant_index`
also projects two direct and one nested explicit `ROW_FIELD` value through
the local planner and the in-process three-worker fragment path, including an
empty worker partition plus independent outer and nested NULL validity, and
compares the typed results and NULL-parent propagation.
External-client wire and cloud qualification for nullable ROW dereference
remain open.
