# Bounded product `UPDATE ... FROM` / `DELETE ... USING` qualification — 2026-11-03

KaveonDB now qualifies a narrow source-backed SQL Server-style `UPDATE ... FROM`
form for product records. It copies a source product document from the pinned
transaction snapshot into one or more target records and commits each
replacement through the existing owner-checked revision/CAS path in one atomic
transaction. The same bounded source predicate is available to
`DELETE ... USING`, which stages revision-checked deletes after validating the
source owner.

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
source must be one aliased base table, and the assignment must either copy
exactly `source.document_json` or wrap that direct source column in
`CAST(source.document_json AS JSON)`. The predicate must contain a literal
`target.revision` plus either one literal `source.id` and one or more literal
target ids (including an `IN` list capped at 1,000 ids), or an identity join
`target.id = source.id` paired with a literal target-id list. Source and target
rows must be owned by the current principal. Each source document is read from
the transaction snapshot; a document staged earlier in the same transaction is
used without rereading storage. Every target revision advances by one and the
commit remains atomic and replay-safe.

## Evidence

- Parser acceptance and fail-closed cases:
  `parser::tests::accepts_bounded_product_update_from_and_rejects_wider_shapes`.
- Typed source assignment:
  `parser::tests::product_update_from_accepts_a_typed_source_document_assignment`.
- Transaction/API qualification:
  `transaction_api::tests::sql_endpoint_bounded_update_from_copies_source_document_and_revises_target`.
- Batch transaction/API qualification:
  `transaction_api::tests::sql_endpoint_bounded_update_from_batch_is_atomic_and_returns_each_target`.
- Delete transaction/API qualification:
  `transaction_api::tests::sql_endpoint_bounded_delete_using_batch_is_atomic_and_returns_each_target`.
- The API test verifies `RETURNING id, revision, document_json`, the committed
  revision, and the persisted source document copied into the target. The batch
  test verifies two target revisions, both returned rows, and atomic publication
  of the copied document.
- The API single-target test uses `CAST(source.document_json AS JSON)` and
  verifies the same persisted document and revision/CAS result as the direct
  source form. The cast is syntax-only and does not broaden the expression
  contract.
- Focused verification on 2026-11-03:
  `cargo test -p kaveon-sql product_update_from_accepts_a_typed_source_document_assignment`
  and
  `cargo test -p kaveon-server --bin kaveon-server sql_endpoint_bounded_update_from_copies_source_document_and_revises_target`
  each passed; SQL/server Clippy also passed with `-D warnings`.

## Explicit boundary

This does not claim general `UPDATE ... FROM` compatibility. Arbitrary joins,
multiple source tables, cross-kind sources, arbitrary expressions or
assignments, source predicates other than one literal id or the identity join,
per-row revision expressions, lake-table mutation, and unbounded scans remain
rejected. General typed-table DML and broader SQL Server/Fabric/PostgreSQL
parity remain open in the compatibility tracker.

The delete form uses the same limits and snapshot rules:

```sql
DELETE FROM product.datasets AS target
USING product.datasets AS source
WHERE target.id IN ('target-1', 'target-2')
  AND target.revision = 1
  AND source.id = 'source-1'
RETURNING id, revision;
```

The source is an owner-checked same-kind base table. Arbitrary `USING` joins,
cross-kind sources, unbounded target scans, and per-row revision expressions
remain rejected.
