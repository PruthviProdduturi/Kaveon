# Bounded SQL Server `MERGE OUTPUT` qualification — 2026-11-03

## Scope

The native transaction API accepts SQL Server's `OUTPUT` spelling for the
already-qualified product-record update-or-insert, matched-update-only,
unmatched-insert-only, and delete-or-insert `MERGE` shapes. It is a bounded result projection, not general SQL Server
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
`inserted.*` are accepted. Matched-delete-only and delete-or-insert batches
also accept `deleted.id` and `deleted.revision`. Unmatched-insert-only batches
also accept the `inserted` projections; source rows that already match are
no-ops and produce no output row. For delete-or-insert, `inserted` returns only
inserted/update actions and `deleted` returns only deleted/update actions. Rows
retain the source `VALUES` order. The result
is read from the same staged revision/CAS snapshot used by the transaction;
updates report the incremented revision and inserts report revision one.
For matched-update-only batches, unmatched source rows are no-ops and produce
no output row; matched rows return the staged replacement document and
incremented revision. For matched-delete-only batches, unmatched source rows
are no-ops and matched rows return the deleted identity and prior revision.

## Safety boundary

The parser and endpoint reject `deleted.document_json`, `$action`, expressions,
`OUTPUT INTO`, mixed pseudo-table projections, and unsupported columns. Bounded
by-source delete batches accept `OUTPUT deleted.id` and
`OUTPUT deleted.revision`; by-source inserted output, `deleted.document_json`,
and broader by-source combinations remain rejected. The transaction API
resolves supported MERGE actions to revision/CAS commands, then filters the
staged write set by the requested pseudo-table; source rows that are no-ops are
omitted rather than fabricated.

## Evidence

- `parser::tests::sql_server_merge_output_lowers_to_inserted_product_returning`
- `parser::tests::sql_server_matched_update_output_lowers_to_inserted_product_returning`
- `parser::tests::sql_server_merge_output_rejects_delete_actions`
- `transaction_api::tests::sql_endpoint_bounded_merge_accepts_inserted_output`
- `transaction_api::tests::sql_endpoint_bounded_merge_matched_update_accepts_inserted_output`
- `transaction_api::tests::sql_endpoint_bounded_merge_matched_delete_accepts_deleted_output`
- `transaction_api::tests::sql_endpoint_bounded_merge_unmatched_insert_accepts_inserted_output`
- `transaction_api::tests::sql_endpoint_bounded_merge_delete_or_insert_output_is_action_aware`
- `transaction_api::tests::sql_endpoint_merge_by_source_output_returns_deleted_identity`
- `cargo test -p kaveon-sql --lib sql_server_merge_output -- --nocapture` — 2 passed
- `cargo test -p kaveon-server --bin kaveon-server transaction_api::tests::sql_endpoint_bounded_merge -- --nocapture` — 12 passed

This record qualifies local parser and transaction-API behavior. It does not
claim cloud, external-client, or general lake-table DML qualification.
