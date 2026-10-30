# Bounded native `MERGE` qualification — 2026-10-29

## Status

Qualified for the KaveonDB product-record transaction surface. This is not a
claim of general SQL Server or PostgreSQL `MERGE` compatibility.

## Contract

The accepted shape is exactly one target product table with an alias, one
aliased derived `VALUES` batch of at most 1,000 rows with columns
`(id, document_json)`, and an
equality predicate between the target and source `id` columns:

```sql
MERGE INTO product.datasets AS target
USING (VALUES
  ('ds-1', '{"name":"Orders"}'),
  ('ds-2', '{"name":"Customers"}')
) AS source (id, document_json)
  ON target.id = source.id
WHEN MATCHED THEN
  UPDATE SET document_json = source.document_json
WHEN NOT MATCHED THEN
  INSERT (id, document_json)
  VALUES (source.id, source.document_json)
```

The parser lowers each source row to the existing owner-isolated `Upsert`
command. A second qualified form uses `WHEN MATCHED THEN DELETE` followed by
the same not-matched insert; the transaction layer resolves each source key
against the pinned snapshot and stages either a revision-checked delete or a
new create. Commit, revision-CAS, replay, ownership, and transaction
visibility therefore use the same tested paths as the bounded conflict-upsert
operation.

A third bounded form supports `WHEN MATCHED THEN DELETE` without an insert
clause. Existing source keys are deleted with the same revision-CAS and
ownership checks; source keys with no match are explicit transactional no-ops,
as required by `MERGE` semantics. The source still carries the fixed
two-column `(id, document_json)` shape because the parser uses one bounded
batch contract for all product-record `MERGE` forms; the document value is
ignored for this delete-only form.

A fourth bounded form supports `WHEN NOT MATCHED THEN INSERT` without a
matched clause. New keys use the normal product validation and revision-one
create path; keys already present in the pinned snapshot are idempotent
no-ops, so retries cannot overwrite existing product state.

The SQL Server spelling `WHEN NOT MATCHED BY TARGET` is accepted as an alias
for the same not-matched clause. Source ids must be unique within the bounded
batch; duplicate keys fail closed before any write is staged, avoiding
order-dependent revisions inside one transaction.

## Rejections

The parser fails closed for source-query inputs, joins, batches over 1,000 rows,
predicates on `WHEN` clauses, non-`id` match keys, arbitrary assignments,
other clause orderings, and non-product targets. General typed lake-table DML,
constraints, named conflict targets, and full `MERGE` semantics remain open.

## Evidence

- `parser::tests::product_merge_supports_bounded_update_or_insert`
- `parser::tests::product_merge_expands_a_bounded_values_batch`
- `parser::tests::product_merge_supports_bounded_delete_or_insert`
- `parser::tests::product_merge_supports_bounded_matched_delete`
- `parser::tests::product_merge_supports_bounded_unmatched_insert`
- `parser::tests::product_merge_rejects_unbounded_or_ambiguous_shapes`
- `parser::tests::rejects_duplicate_merge_source_ids`
- `transaction_api::tests::sql_endpoint_bounded_merge_batch_updates_and_inserts_atomically`
- `transaction_api::tests::sql_endpoint_bounded_merge_delete_or_insert_is_owner_checked_and_atomic`
- `transaction_api::tests::sql_endpoint_bounded_merge_matched_delete_is_atomic_and_unmatched_is_noop`
- `transaction_api::tests::sql_endpoint_bounded_merge_unmatched_insert_is_idempotent`
- `cargo test -p kaveon-sql --lib product_merge_` — 6 passed, 0 failed on 2026-10-30
- `cargo test -p kaveon-server --bin kaveon-server transaction_api::tests::sql_endpoint_bounded_merge` — 4 passed, 0 failed on 2026-10-30

The transaction API test commits two revisions and one insert atomically, then
reads the committed snapshot and verifies revisions `2, 2, 1`. It stages the
adapted commands through the same `ProductDmlCommand::Upsert` path used by the
qualified conflict-upsert tests.
