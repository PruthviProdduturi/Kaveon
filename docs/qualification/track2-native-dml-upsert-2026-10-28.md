# Bounded product upsert qualification — 2026-10-28

## Contract

Kaveon qualifies one intentionally narrow upsert form for product records and
the same revisioned form for `kaveon.product.typed_rows`:

```sql
INSERT INTO product.datasets (id, document_json)
VALUES ('orders', '{"name":"Updated"}')
ON CONFLICT (id)
DO UPDATE SET document_json = excluded.document_json
RETURNING id, revision;
```

The parser accepts only a product or typed-row target, a single `id` conflict
key, one assignment to `document_json` from `excluded.document_json`, and no
conflict predicate. The server applies the owner check and revision CAS on
update. A missing id follows the create path at revision 1; an existing owned
id advances its revision by one. Typed-row upserts cannot change the row's
declared table. Arbitrary assignments, named constraints, conflict predicates,
and arbitrary conflict targets fail closed. This remains a typed-row facade,
not general lake-table DML.

## Evidence

- SQL parser unit test: `product_insert_supports_bounded_on_conflict_do_update`.
- Server transaction API test:
  `sql_endpoint_on_conflict_do_update_revises_owned_product`.
- SQL parser test: `typed_row_insert_supports_bounded_conflict_update`.
- Server transaction test:
  `typed_row_transactions_are_owner_isolated_and_rollback_is_invisible`.
- The API test creates `orders` at revision 1, executes the bounded upsert,
  verifies `RETURNING id, revision` reports revision 2, commits, and verifies
  the committed product record is revision 2.
- Full SQL crate suite: 279 passed, 0 failed.
- Targeted server upsert test: passed.

This is a qualified product-transaction increment, not general PostgreSQL
`ON CONFLICT` or `MERGE` compatibility. Source-backed `INSERT … SELECT`,
general typed-table DML, and broader conflict expressions remain open work.
