# Bounded native `MERGE` qualification — 2026-10-28

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
command. Commit, revision-CAS, replay, and transaction visibility therefore
use the same tested path as the bounded `ON CONFLICT (id) DO UPDATE` operation.

## Rejections

The parser fails closed for source-query inputs, joins, batches over 1,000 rows,
predicates on `WHEN` clauses, non-`id` match keys, deletes, arbitrary
assignments, other clause orderings, and non-product targets. General typed lake-table DML,
constraints, named conflict targets, and full `MERGE` semantics remain open.

## Evidence

- `parser::tests::product_merge_supports_bounded_update_or_insert`
- `parser::tests::product_merge_expands_a_bounded_values_batch`
- `parser::tests::product_merge_rejects_unbounded_or_ambiguous_shapes`
- `transaction_api::tests::sql_endpoint_bounded_merge_batch_updates_and_inserts_atomically`
- `cargo test -p kaveon-sql` — 285 passed, 0 failed on 2026-10-28

The transaction API test commits two revisions and one insert atomically, then
reads the committed snapshot and verifies revisions `2, 2, 1`. It stages the
adapted commands through the same `ProductDmlCommand::Upsert` path used by the
qualified conflict-upsert tests.
