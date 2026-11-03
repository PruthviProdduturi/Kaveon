# Bounded `MERGE` matched predicates — 2026-11-03

KaveonDB now qualifies a narrow predicate form for product-record `MERGE`
statements:

```sql
MERGE INTO product.datasets AS target
USING (VALUES ('dataset-a', '{"v":1}'), ('dataset-b', '{"v":2}'))
  AS source (id, document_json)
ON target.id = source.id
WHEN MATCHED AND source.id IN ('dataset-b')
  THEN UPDATE SET document_json = source.document_json;
```

The supported predicate is limited to `source.id = 'literal'` or
`source.id IN ('literal', ...)`, and only a single matched-only `UPDATE` or
`DELETE` clause may use it. The parser filters the bounded source batch before
creating the normal revision/CAS commands. The transaction layer still checks
ownership, resolves each target against the pinned snapshot, and commits the
selected rows atomically. Rows excluded by the predicate and source IDs with
no matching target are no-ops.

Evidence:

- `engine/crates/sql/src/parser.rs::product_merge_supports_bounded_matched_source_filter`
- `engine/crates/server/src/transaction_api.rs::sql_endpoint_bounded_merge_matched_update_is_cas_checked_and_unmatched_is_noop`
- implementation commit `ce916270`;
- parser, API transaction, clippy, and formatting gates pass.

Predicates on multi-clause `MERGE`, target columns, arbitrary expressions,
`NOT IN`, and general `MERGE` search conditions remain explicitly rejected.
This is a bounded KaveonDB transaction qualification, not unrestricted SQL
Server, PostgreSQL, or Fabric `MERGE` compatibility.
