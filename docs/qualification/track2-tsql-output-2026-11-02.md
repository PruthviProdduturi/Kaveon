# Bounded SQL Server `OUTPUT` qualification — 2026-11-02

## Contract

The native transaction parser accepts the SQL Server `OUTPUT` spelling for
product-record `INSERT`, `UPDATE`, and `DELETE` when every projection is a
simple statement pseudo-table column:

```sql
UPDATE product.datasets AS target
SET document_json = '{"name":"Orders"}'
OUTPUT inserted.id, inserted.revision
WHERE id = 'orders' AND revision = 1
```

For deletion, `deleted.id`, `deleted.revision`, and
`deleted.document_json` map to the existing revision/CAS `RETURNING` result
path. The normalization is syntactic; it does not change ownership,
revision, transaction, or result semantics.

## Evidence

- `parser::tests::sql_server_output_lowers_to_bounded_product_returning`
- `parser::tests::sql_server_output_rejects_expressions_and_output_into`
- `transaction_api::tests::sql_endpoint_product_insert_accepts_sql_server_output`
- `transaction_api::tests::sql_endpoint_update_and_delete_return_identity_and_revision`
- `cargo test -p kaveon-sql --lib sql_server_output` — 2 passed
- `cargo test -p kaveon-server sql_endpoint_update_and_delete_return_identity_and_revision` — passed
- `cargo test -p kaveon-server sql_endpoint_product_insert_accepts_sql_server_output` — passed
- `cargo clippy -p kaveon-sql -p kaveon-server --all-targets --no-default-features -- -D warnings` — passed

## Rejections

Expressions, aliases, `OUTPUT INTO`, `MERGE OUTPUT`, mixed inserted/deleted
projections, arbitrary table DML, and unsupported columns remain fail-closed.
