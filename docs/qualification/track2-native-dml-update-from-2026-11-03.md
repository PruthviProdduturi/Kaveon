# Bounded product `UPDATE ... FROM` qualification — 2026-11-03

KaveonDB now qualifies a narrow source-backed SQL Server-style `UPDATE ... FROM`
form for product records. It copies one source product document from the pinned
transaction snapshot into one target record and commits the replacement through
the existing owner-checked revision/CAS path.

## Qualified shape

```sql
UPDATE product.datasets AS target
SET target.document_json = source.document_json
FROM product.datasets AS source
WHERE target.id = 'target-1'
  AND target.revision = 1
  AND source.id = 'source-1'
RETURNING id, revision, document_json;
```

The target and source must be the same supported `kaveon.product` kind. The
source must be one aliased base table, the assignment must copy exactly
`source.document_json`, and the predicate must contain exactly one literal
`target.id`, `target.revision`, and `source.id` equality. Source and target rows
must be owned by the current principal. The source document is read from the
transaction snapshot; a document staged earlier in the same transaction is
used without rereading storage. The target revision advances by one and the
commit remains atomic and replay-safe.

## Evidence

- Parser acceptance and fail-closed cases:
  `parser::tests::accepts_bounded_product_update_from_and_rejects_wider_shapes`.
- Transaction/API qualification:
  `transaction_api::tests::sql_endpoint_bounded_update_from_copies_source_document_and_revises_target`.
- The API test verifies `RETURNING id, revision, document_json`, the committed
  revision, and the persisted source document copied into the target.

## Explicit boundary

This does not claim general `UPDATE ... FROM` compatibility. Joins, multiple
source rows, cross-kind sources, arbitrary expressions or assignments, source
predicates other than one literal id, per-row revision expressions, lake-table
mutation, and unbounded scans remain rejected. General typed-table DML and
broader SQL Server/Fabric/PostgreSQL parity remain open in the compatibility
tracker.
