# Bounded product DML ID-list CAS — 2026-11-02

## Contract

KaveonDB product `UPDATE` and `DELETE` accept a bounded explicit ID list:

```sql
UPDATE product.datasets
SET document_json = '{"published":true}'
WHERE id IN ('one', 'two') AND revision = 3;

DELETE FROM product.datasets
WHERE revision = 3 AND id IN ('one', 'two');
```

The list contains one to 1,000 string IDs, contains no duplicates, and must be
paired with one positive revision CAS value. The adapter expands the statement
into one owner-isolated transactional command per ID. The existing revision,
ownership, audit, commit, and rollback rules therefore apply independently to
each record under the same transaction boundary. Arbitrary expressions,
`NOT IN`, per-row revisions, joins, and `UPDATE ... FROM` remain rejected.

## Evidence

- Parser coverage: `product_update_and_delete_expand_bounded_id_lists_with_one_cas_revision`.
- Transaction/API coverage:
  `sql_endpoint_expands_bounded_product_id_list_dml_under_one_cas_revision`.
- The transaction test creates two product records, updates both with one
  revision predicate, commits atomically, and verifies revision `2` plus the
  new documents through the owner-scoped read path.
- A failed list containing one valid and one missing ID is fail-closed: the
  session restores its pre-batch staged view, `COMMIT` cannot publish the first
  command, and the durable record remains at revision `1` with its original
  document.
- Duplicate IDs and `revision IN (...)` fail closed before staging.

This is a bounded KaveonDB product-record contract, not general lake-table
mutation or a claim of unrestricted PostgreSQL/Fabric DML parity.
