# Bounded `MERGE` not-matched-by-source update qualification — 2026-11-03

## Contract

KaveonDB accepts the bounded SQL Server form:

```sql
MERGE INTO product.datasets AS target
USING (VALUES ('keep-me', '{"kept":true}')) AS source (id, document_json)
  ON target.id = source.id
WHEN MATCHED THEN
  UPDATE SET document_json = source.document_json
WHEN NOT MATCHED THEN
  INSERT (id, document_json) VALUES (source.id, source.document_json)
WHEN NOT MATCHED BY SOURCE THEN
  UPDATE SET document_json = '{"archived":true}'
```

The source remains a two-column `VALUES` batch of at most 1,000 unique rows.
The by-source update value must be one string literal assigned to
`document_json`; arbitrary expressions, predicates, joins, source tables, and
typed lake-table DML remain rejected. The transaction layer evaluates target
rows against the transaction's pinned snapshot, excludes source IDs, enforces
owner isolation, caps the expanded affected set at 1,000 rows, and stages each
replacement through revision CAS. Unmatched source rows still follow the
existing insert/update contract.

## Evidence

- `parser::tests::product_merge_supports_bounded_not_matched_by_source_update`
- `transaction_api::tests::sql_endpoint_bounded_merge_not_matched_by_source_updates_owned_rows`
- `cargo test -p kaveon-sql product_merge_supports_bounded_not_matched_by_source_update`
  — passed
- `cargo test -p kaveon-server --bin kaveon-server sql_endpoint_bounded_merge_not_matched_by_source_updates_owned_rows`
  — passed

## Boundary

This is a bounded product-record transaction extension, not general SQL Server
`MERGE` parity. General assignments, source-backed `MERGE`, arbitrary target
tables, named constraints, and cross-source mutation remain outside the
qualified contract.
