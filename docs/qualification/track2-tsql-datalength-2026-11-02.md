# T-SQL `DATALENGTH` qualification — 2026-11-02

## Contract

Kaveon accepts the bounded SQL Server spelling:

```sql
SELECT DATALENGTH(text_column) FROM table_name;
```

The result is a nullable `BIGINT` containing the UTF-8 byte length of each
text value, including trailing spaces. `NULL` remains `NULL`. This is the
explicit Kaveon text contract; it does not claim SQL Server's UTF-16 storage
width for `nvarchar` values. The function works for plain and dictionary/
`LargeUtf8` text arrays and is limited to one scalar argument.

## Evidence

- Planner lowering: `logical_plan::tests::lowers_tsql_datalength_as_bounded_scalar_function`.
- Local evaluator: `expr_eval::tests::evaluates_basic_text_mappers_for_large_utf8`,
  including trailing spaces, multibyte `λ`, and NULL propagation.
- Three-worker/layout differential: `KAVEON_DIFF_ONLY=tsql_datalength cargo test
  -p kaveon-server --bin kaveon-server differential_tests --quiet` — 197 tests
  passed, including dictionary, plain, multi-file, partitioned, clustered, and
  three-worker fragment paths; the selected case returned exact matching rows.
- Regression suites after the implementation: `cargo test -p kaveon-sql --lib
  --quiet` — 347 passed; `cargo test -p kaveon-exec --lib --quiet` — 615 passed,
  4 ignored.
- Full bounded corpus entry: `datalength` in
  `tsql-read-compatibility-corpus-2026-10-28.json`, pinned to implementation
  commit `ff7d17be`.

## Boundary

Binary values, SQL Server code-page/UTF-16 byte emulation, and arbitrary
multi-argument forms remain outside this qualification.
