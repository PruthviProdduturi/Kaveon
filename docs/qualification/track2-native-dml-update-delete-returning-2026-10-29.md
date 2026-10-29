# Native product DML `UPDATE`/`DELETE RETURNING` qualification — 2026-10-29

## Scope

This qualification covers the bounded KaveonDB product-record contract for:

```sql
UPDATE product.datasets
SET document_json = '{"name":"Orders"}'
WHERE id = 'orders' AND revision = 1
RETURNING id, revision;

DELETE FROM product.datasets
WHERE id = 'orders' AND revision = 2
RETURNING id, revision;
```

The statements operate only on supported product metadata families. They do
not add general lake-table DML, arbitrary typed assignments, `FROM`/`USING`,
or arbitrary `RETURNING` expressions.

## Evidence

- SQL parser regression: `accepts_identity_returning_for_product_update_and_delete`
- Transaction API regression: `sql_endpoint_update_and_delete_return_identity_and_revision`
- `cargo test -p kaveon-sql --lib accepts_identity_returning_for_product_update_and_delete -- --nocapture`
- `cargo test -p kaveon-server sql_endpoint_update_and_delete_return_identity_and_revision -- --nocapture`

Both tests passed on commit `PENDING_COMMIT`.

## Contract

- `UPDATE` and `DELETE` require an exact `id` plus positive `revision` predicate.
- `UPDATE` may replace only `document_json`.
- `RETURNING` may name only unqualified `id` and `revision`.
- The write remains owner-isolated and revision-checked.
- Returned update revision is the expected revision plus one.
- Returned delete revision is the deleted record's expected revision.
- Staging and commit behavior is unchanged: the response is from the staged
  snapshot, and `COMMIT` publishes the immutable product snapshot.
- Unsupported shapes continue to fail closed.
