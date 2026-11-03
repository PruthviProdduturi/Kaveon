# Bounded product UPDATE and DELETE target aliases — 2026-11-02

## Contract

Kaveon accepts a SQL Server-style alias on the single product-table target of
the existing revision/CAS `UPDATE` and `DELETE` contracts:

```sql
UPDATE product.datasets AS target
SET document_json = '{"name":"Orders"}'
WHERE id = 'orders' AND revision = 1
RETURNING id, revision
```

The alias does not widen the write surface. The statement still requires one
product table, a revision predicate, owner checks, simple assignments, and the
existing staged `RETURNING` columns.

The same target alias form is accepted for revision-checked deletion:

```sql
DELETE FROM product.datasets AS target
WHERE id = 'orders' AND revision = 2
RETURNING id, revision, document_json
```

## Evidence

- `parser::tests::product_update_accepts_a_target_alias_without_widening_the_contract`
- `parser::tests::accepts_identity_returning_for_product_update_and_delete`
- `transaction_api::tests::sql_endpoint_update_and_delete_return_identity_and_revision`
- `cargo test -p kaveon-sql --lib product_update_accepts_a_target_alias_without_widening_the_contract` — 1 passed
- `cargo test -p kaveon-server sql_endpoint_update_and_delete_return_identity_and_revision` — passed

## Rejections

Target aliases with column aliases, joins, `UPDATE ... FROM`, qualified
assignments, and arbitrary lake-table DML remain fail-closed.
