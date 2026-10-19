# Track 2 native product DML constant `INSERT ... SELECT` — 2026-10-18

Status: **qualified for the declared product-record transaction facade**.

## Shape qualified

The product transaction API accepts one constant `SELECT` as the source of an
`INSERT` into a supported product table:

```sql
INSERT INTO product.datasets (id, document_json)
SELECT 'orders', '{"name":"Orders"}'
```

The projection must contain exactly the two supported literal values. Column
order is resolved by name, so `(document_json, id)` is also valid. The
statement is converted into the same typed product-record create command used
by `VALUES`, then staged and committed under the normal snapshot/CAS boundary.

## Evidence

- `kaveon-sql` parser tests cover the accepted constant projection and the
  fail-closed rejection of `FROM`, `DISTINCT`, and `LIMIT` forms.
- `kaveon-server` transaction API coverage stages and commits the statement,
  then reads the product document back through the owner-checked registry.
- Existing product-record validation, JSON document checks, transaction
  ownership, revision handling, and commit idempotency remain unchanged.

## Boundary

This is a bounded compatibility slice for product metadata. It does not scan a
source table and does not implement general `INSERT ... SELECT` into lake
tables. CTEs, joins, expressions, aggregates, conflict clauses, `RETURNING`,
generated columns, constraints, and arbitrary typed assignments remain
unsupported and fail closed. Distributed workers are not involved because
product-record transactions are committed by the transactional catalog path.
