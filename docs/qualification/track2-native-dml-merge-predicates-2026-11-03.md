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

The supported filtering predicate is limited to `source.id = 'literal'` or
`source.id IN ('literal', ...)`, and only a single matched-only `UPDATE` or
`DELETE` clause may use it. In addition, the first `WHEN MATCHED` clause may
carry the redundant identity predicate `target.id = source.id` (in either
operand order), including in the bounded update-or-insert and not-matched-by-
source forms. The `ON` equality already guarantees that predicate, so it is
accepted as a deterministic no-op rather than evaluated as a general search
condition. The parser filters the bounded source batch before creating the
normal revision/CAS commands. The transaction layer still checks ownership,
resolves each target against the pinned snapshot, and commits the selected rows
atomically. Rows excluded by a source filter and source IDs with no matching
target are no-ops.

Insert-only `WHEN NOT MATCHED` clauses may also carry the same bounded
`source.id = 'literal'` or `source.id IN (...)` predicate. Because no matched
action exists in that form, filtering the source batch is equivalent to the
SQL clause and preserves the normal idempotent create-if-absent behavior.

When the target alias is declared, the equivalent target-side forms
`target.id = 'literal'` and `target.id IN ('literal', ...)` are also supported.
Because the required `ON target.id = source.id` condition is an identity
equality, the target-side form selects the same bounded source rows; it does
not introduce a general target search predicate or a second mutation path.
Selected rows still use the normal owner, pinned-snapshot, and revision/CAS
checks.

Evidence:

- `engine/crates/sql/src/parser.rs::product_merge_supports_bounded_matched_source_filter`
- `engine/crates/sql/src/parser.rs::product_merge_supports_bounded_matched_delete_source_filter`
- `engine/crates/server/src/transaction_api.rs::sql_endpoint_bounded_merge_batch_updates_and_inserts_atomically`
- `engine/crates/server/src/transaction_api.rs::sql_endpoint_bounded_merge_matched_update_is_cas_checked_and_unmatched_is_noop`
- `engine/crates/server/src/transaction_api.rs::sql_endpoint_bounded_merge_matched_delete_is_atomic_and_unmatched_is_noop`
- `engine/crates/server/src/transaction_api.rs::sql_endpoint_bounded_merge_matched_update_is_cas_checked_and_unmatched_is_noop` (target-side filter)
- `engine/crates/server/src/transaction_api.rs::sql_endpoint_bounded_merge_unmatched_insert_is_idempotent`
- implementation commit `2daaab6e`;
- parser, API transaction, clippy, and formatting gates pass.

Predicates other than the bounded source/target `id` filters and the redundant
identity predicate on the first matched clause, target filters combined with
other conditions, arbitrary expressions, `NOT IN`, and general `MERGE` search
conditions remain explicitly rejected.
This is a bounded KaveonDB transaction qualification, not unrestricted SQL
Server, PostgreSQL, or Fabric `MERGE` compatibility.
