# Bounded `MERGE` filtered update-or-insert qualification — 2026-11-04

## Contract

KaveonDB accepts the bounded SQL Server form:

```sql
MERGE INTO product.datasets AS target
USING (VALUES
  ('existing', '{"changed":true}'),
  ('new-row', '{"created":true}')
) AS source (id, document_json)
ON target.id = source.id
WHEN MATCHED AND source.id IN ('existing')
  THEN UPDATE SET document_json = source.document_json
WHEN NOT MATCHED
  THEN INSERT (id, document_json) VALUES (source.id, source.document_json)
```

The source is a bounded `VALUES` batch of at most 1,000 unique IDs. The
matched predicate is limited to a literal `source.id = '…'` or
`source.id IN ('…', …)` filter (the equivalent qualified `target.id` form is
also accepted). Existing target rows outside that filter are transactional
no-ops; absent source rows still take the `WHEN NOT MATCHED` insert action.
Each selected update uses the normal owner check and revision/CAS write path,
and each insert uses the normal create path. Arbitrary predicates, joins,
assignments, scans, and filtered matched DELETE paired with an insert clause
remain rejected.

## Evidence

- Parser adaptation: `product_merge_supports_filtered_matched_update_with_insert_fallback`
- Transaction API: `sql_endpoint_bounded_merge_filtered_match_updates_and_inserts`
- Broader parser MERGE suite: 19 passed
- Transaction API bounded MERGE suite: 13 passed
- `cargo check -p kaveon-sql -p kaveon-server --all-targets` passed

## Boundary

This is a bounded KaveonDB product-record contract, not general lake-table
mutation or PostgreSQL-compatible DML. The source batch and affected-row cap,
owner isolation, pinned snapshot, and revision/CAS semantics remain required.
