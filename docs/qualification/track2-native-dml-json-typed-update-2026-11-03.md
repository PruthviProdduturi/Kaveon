# Native DML JSON-typed update qualification — 2026-11-03

The bounded KaveonDB product `UPDATE` contract now accepts either the existing
JSON string literal or a typed JSON literal:

```sql
UPDATE product.datasets
SET document_json = CAST('{"name":"Orders"}' AS JSON)
WHERE id = 'orders' AND revision = 1;
```

Only a literal string inside the cast is accepted. Arbitrary expressions,
subqueries, joins, and general typed-column assignments remain rejected. The
transaction layer still parses the bytes as a JSON object and applies the
existing owner check, revision CAS, immutable publication, audit event, and
`RETURNING` behavior.

Evidence:

```text
cargo test -p kaveon-sql --lib product_update_accepts_a_json_typed_document_literal -- --nocapture
cargo test -p kaveon-server --bin kaveon-server sql_endpoint_update_and_delete_return_identity_and_revision -- --nocapture
```

This is a product-record DML extension, not general lake-table mutation or a
claim of PostgreSQL-compatible arbitrary assignments.
