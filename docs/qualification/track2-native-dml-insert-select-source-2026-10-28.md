# Bounded product source `INSERT … SELECT` qualification

Date: 2026-10-28  
Scope: KaveonDB product-record transaction API  
Status: qualified (bounded)

Kaveon now supports the following source-backed form:

```sql
INSERT INTO product.datasets (id, document_json)
SELECT id, document_json
FROM product.datasets
ON CONFLICT (id) DO UPDATE
SET document_json = excluded.document_json;
```

The document projection may also use the direct typed form
`CAST(document_json AS JSON)`; this is syntax-only and preserves the same
source snapshot, owner filtering, and conflict/CAS behavior.

The parser and transaction layer intentionally keep this contract narrow:

- exactly one `kaveon.product` source table;
- source and target are the same product-record kind;
- direct, unaliased `id` projection and either a direct `document_json`
  projection or `CAST(document_json AS JSON)`;
- no joins, CTEs, ordering, grouping, or limits; the only predicate allowed is
  a literal `id IN (...)` list of at most 1,000 values. With that bounded
  predicate, `RETURNING id, revision, document_json` is supported; unbounded
  source-copy `RETURNING` remains rejected;
- source rows are read from the transaction's pinned catalog snapshot;
- the source is owner-filtered, with a hard cap of 1,000 rows per statement;
- conflict handling reuses the existing `DO NOTHING` or revision-CAS upsert path.

Evidence:

- parser accepts the bounded form and rejects joins, aliases, modifiers, and
  unbounded `RETURNING` (`cargo test -p kaveon-sql product_insert_select`);
- the server API seeds a product record, executes the source copy with the
  revision-CAS conflict form, returns the committed identity, revision, and
  document JSON, commits, and verifies revision 2 and document equality
  (`transaction_api::tests::sql_endpoint_commits_bounded_product_table_insert_select`).
- Focused verification on 2026-11-03 passed the parser and server API tests with
  the typed document projection, plus SQL/server Clippy with `-D warnings`.

This is not general `INSERT … SELECT` over Parquet/Delta or registered
external sources. Cross-kind copies, arbitrary expressions beyond the direct
JSON cast, filters, joins,
lake-table writes, and general typed-table DML remain explicitly outside the
qualified boundary.
