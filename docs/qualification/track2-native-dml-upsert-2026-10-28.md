# Bounded product upsert qualification — 2026-10-28

## Contract

Kaveon qualifies one intentionally narrow upsert form for product records:

```sql
INSERT INTO product.datasets (id, document_json)
VALUES ('orders', '{"name":"Updated"}')
ON CONFLICT (id)
DO UPDATE SET document_json = excluded.document_json
RETURNING id, revision;
```

The parser accepts only a product record target, a single `id` conflict key,
one assignment to `document_json` from `excluded.document_json`, and no conflict
predicate. The server applies the owner check and revision CAS on update. A
missing id follows the create path at revision 1; an existing owned id advances
its revision by one. Arbitrary assignments, named constraints, conflict
predicates, and arbitrary conflict targets fail closed. Typed lake rows are not
included in this contract.

## Evidence

- SQL parser unit test: `product_insert_supports_bounded_on_conflict_do_update`.
- Server transaction API test:
  `sql_endpoint_on_conflict_do_update_revises_owned_product`.
- The API test creates `orders` at revision 1, executes the bounded upsert,
  verifies `RETURNING id, revision` reports revision 2, commits, and verifies
  the committed product record is revision 2.
- Full SQL crate suite: 279 passed, 0 failed.
- Targeted server upsert test: passed.

This is a qualified product-transaction increment, not general PostgreSQL
`ON CONFLICT` or `MERGE` compatibility. Source-backed `INSERT … SELECT`,
general typed-table DML, and broader conflict expressions remain open work.
