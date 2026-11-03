# Bounded SQL Server `MERGE OUTPUT` qualification — 2026-11-03

## Scope

The native transaction API now accepts SQL Server's `OUTPUT` spelling for the
already-qualified product-record update-or-insert, matched-update-only, and
unmatched-insert-only `MERGE` shapes. It is a bounded result projection, not general SQL Server
`MERGE` or `$action` parity.

```sql
MERGE INTO product.datasets AS target
USING (VALUES ('existing', '{"updated":true}'), ('new', '{"created":true}'))
  AS source (id, document_json)
ON target.id = source.id
WHEN MATCHED THEN
  UPDATE SET document_json = source.document_json
WHEN NOT MATCHED THEN
  INSERT (id, document_json) VALUES (source.id, source.document_json)
OUTPUT inserted.*
```

`inserted.id`, `inserted.revision`, `inserted.document_json`, and
`inserted.*` are accepted. Homogeneous matched-delete-only batches also accept
`deleted.id` and `deleted.revision`. Homogeneous unmatched-insert-only batches
also accept the `inserted` projections; source rows that already match are
no-ops and produce no output row. Rows retain the source `VALUES` order. The result
is read from the same staged revision/CAS snapshot used by the transaction;
updates report the incremented revision and inserts report revision one.
For matched-update-only batches, unmatched source rows are no-ops and produce
no output row; matched rows return the staged replacement document and
incremented revision. For matched-delete-only batches, unmatched source rows
are no-ops and matched rows return the deleted identity and prior revision.

## Safety boundary

The parser and endpoint reject `deleted.document_json`, `$action`, expressions,
`OUTPUT INTO`, mixed pseudo-tables, and unsupported columns. A `MERGE` whose
expanded commands include delete-or-insert or
not-matched-by-source actions rejects `OUTPUT` before staging. This avoids
inventing action identity or returning a misleading inserted row for a delete.
Those shapes remain open until the transaction API has an ordered,
action-aware batch-result contract.

## Evidence

- `parser::tests::sql_server_merge_output_lowers_to_inserted_product_returning`
- `parser::tests::sql_server_matched_update_output_lowers_to_inserted_product_returning`
- `parser::tests::sql_server_merge_output_rejects_delete_actions`
- `transaction_api::tests::sql_endpoint_bounded_merge_accepts_inserted_output`
- `transaction_api::tests::sql_endpoint_bounded_merge_matched_update_accepts_inserted_output`
- `transaction_api::tests::sql_endpoint_bounded_merge_matched_delete_accepts_deleted_output`
- `transaction_api::tests::sql_endpoint_bounded_merge_unmatched_insert_accepts_inserted_output`
- `cargo test -p kaveon-sql --lib sql_server_merge_output -- --nocapture` — 2 passed
- `cargo test -p kaveon-server --bin kaveon-server transaction_api::tests::sql_endpoint_bounded_merge -- --nocapture` — 11 passed

This record qualifies local parser and transaction-API behavior. It does not
claim cloud, external-client, or general lake-table DML qualification.
