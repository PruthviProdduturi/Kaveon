# Track 2 JSON typed-return qualification

Date: 2026-10-18

The SQL parser now accepts the bounded Trino SQL/JSON typed-return forms:

```sql
JSON_VALUE(document, path RETURNING BIGINT)
JSON_QUERY(document, path RETURNING JSON)
```

The parser lowers these forms to the existing typed `CAST` path after JSON
extraction. The allow-list is deliberately limited to `BIGINT`, `INTEGER`,
`DOUBLE`, `REAL`, `BOOLEAN`, `VARCHAR`, and `JSON`; unsupported return types,
including `DECIMAL(10, 2)`, remain rejected rather than being silently coerced.
Nested JSON calls and string literals containing the word `RETURNING` are
covered so the rewrite does not alter JSON payloads or SQL literals.

Evidence:

- `cargo test -p kaveon-sql --quiet`: 204 passed.
- `cargo test -p kaveon-exec json --quiet`: 20 passed.
- `cargo clippy -p kaveon-sql --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- Server differential corpus includes `json_scalar_path_returning_bigint` and
  `json_query_path_returning_json`; `cargo check -p kaveon-server --tests`
  passed. Executing the server test binary was blocked by the local Windows
  linker (`LNK1104` while creating the test executable), so live distributed
  results remain a required CI/AKS qualification.

This is a bounded compatibility step. Full SQL/JSON coercion remains open for
`ON EMPTY`, `ON ERROR`, decimal and temporal return types, complete path
semantics, and wider external-client qualification.
