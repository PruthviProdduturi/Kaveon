# Track 2 SQL compatibility qualification — nullable ROW dereference

**Scope:** `ROW_FIELD` dereference of a nullable Arrow `STRUCT`/Trino `ROW`.

When a row value is NULL, dereferencing one of its fields must return NULL,
even if the physical child array contains a value in that slot. Kaveon's
evaluator now applies the parent row validity bitmap to the selected child
column. Non-null rows continue to return the child without an extra copy.

## Validation

From `engine/`:

```powershell
cargo fmt --all -- --check
cargo test -p kaveon-exec row_field --lib --quiet
cargo clippy -p kaveon-exec --lib -- -D warnings
```

The focused test suite passes both the existing named-field dereference case
and a nullable-parent case whose results are `[7, NULL]`. This is a local
expression qualification; distributed and external-client wire qualification
for nullable ROW dereference remains open.
