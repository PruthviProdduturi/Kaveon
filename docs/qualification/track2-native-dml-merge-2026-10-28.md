# Bounded native `MERGE` qualification — 2026-11-03

## Status

Qualified for the KaveonDB product-record transaction surface. This is not a
claim of general SQL Server or PostgreSQL `MERGE` compatibility.

## Contract

The accepted shape is exactly one target product table with an optional simple
alias, one
aliased derived `VALUES` batch of at most 1,000 rows with columns
`(id, document_json)`, and an
equality predicate between the target and source `id` columns:

```sql
MERGE INTO product.datasets AS target
USING (VALUES
  ('ds-1', '{"name":"Orders"}'),
  ('ds-2', '{"name":"Customers"}')
) AS source (id, document_json)
  ON target.id = source.id
WHEN MATCHED THEN
  UPDATE SET document_json = source.document_json
WHEN NOT MATCHED THEN
  INSERT (id, document_json)
  VALUES (source.id, source.document_json)
```

When no target alias is supplied, the base table name is the only accepted
target qualifier (for example, `datasets.id`); this is equivalent to the
explicit `target` alias and does not broaden the write semantics.

The parser lowers each source row to the existing owner-isolated `Upsert`
command. A second qualified form uses `WHEN MATCHED THEN DELETE` followed by
the same not-matched insert; the transaction layer resolves each source key
against the pinned snapshot and stages either a revision-checked delete or a
new create. Commit, revision-CAS, replay, ownership, and transaction
visibility therefore use the same tested paths as the bounded conflict-upsert
operation.

A third bounded form supports `WHEN MATCHED THEN DELETE` without an insert
clause. Existing source keys are deleted with the same revision-CAS and
ownership checks; source keys with no match are explicit transactional no-ops,
as required by `MERGE` semantics. The source still carries the fixed
two-column `(id, document_json)` shape because the parser uses one bounded
batch contract for all product-record `MERGE` forms; the document value is
ignored for this delete-only form.

A fourth bounded form supports `WHEN NOT MATCHED THEN INSERT` without a
matched clause. New keys use the normal product validation and revision-one
create path; keys already present in the pinned snapshot are idempotent
no-ops, so retries cannot overwrite existing product state.

A fifth bounded form supports `WHEN MATCHED THEN UPDATE` without an insert
clause. Existing source keys resolve to the current owner-checked revision and
stage a compare-and-swap update; source keys with no match are transactional
no-ops. This gives the common update-only `MERGE` shape explicit revision
semantics without pretending to support arbitrary predicates or assignments.
The matched update may either copy `source.document_json` or assign one string
literal to every matched source row; both forms stage the same CAS update and
remain limited to the bounded product document column.

A sixth bounded form adds SQL Server's `WHEN NOT MATCHED BY SOURCE THEN
DELETE` after the matched-update and not-matched-insert clauses. The source ids
are evaluated against the same pinned snapshot; owned target rows absent from
that source set are expanded into revision-checked deletes. The combined
source-plus-delete expansion is capped at 1,000 affected rows, and rows owned
by another principal fail closed rather than being silently removed.

The SQL Server spelling `WHEN NOT MATCHED BY TARGET` is accepted as an alias
for the same not-matched clause. Source ids must be unique within the bounded
batch; duplicate keys fail closed before any write is staged, avoiding
order-dependent revisions inside one transaction.

## Rejections

The parser fails closed for source-query inputs, joins, batches over 1,000 rows,
predicates on `WHEN` clauses, non-`id` match keys, arbitrary assignments,
other clause orderings, and non-product targets. General typed lake-table DML,
constraints, named conflict targets, and full `MERGE` semantics remain open.

## Evidence

- `parser::tests::product_merge_supports_bounded_update_or_insert`
- `parser::tests::product_merge_expands_a_bounded_values_batch`
- `parser::tests::product_merge_supports_bounded_delete_or_insert`
- `parser::tests::product_merge_supports_bounded_matched_delete`
- `parser::tests::product_merge_supports_bounded_matched_update`
- `parser::tests::product_merge_supports_bounded_literal_matched_update`
- `parser::tests::product_merge_supports_bounded_unmatched_insert`
- `parser::tests::product_merge_supports_bounded_not_matched_by_source_delete`
- `parser::tests::product_merge_rejects_unbounded_or_ambiguous_shapes`
- `parser::tests::rejects_duplicate_merge_source_ids`
- `transaction_api::tests::sql_endpoint_bounded_merge_batch_updates_and_inserts_atomically`
- `transaction_api::tests::sql_endpoint_bounded_merge_delete_or_insert_is_owner_checked_and_atomic`
- `transaction_api::tests::sql_endpoint_bounded_merge_matched_delete_is_atomic_and_unmatched_is_noop`
- `transaction_api::tests::sql_endpoint_bounded_merge_unmatched_insert_is_idempotent`
- `transaction_api::tests::sql_endpoint_bounded_merge_matched_update_is_cas_checked_and_unmatched_is_noop`
- `transaction_api::tests::sql_endpoint_bounded_merge_not_matched_by_source_deletes_owned_rows`
- `parser::tests::product_merge_accepts_an_unaliased_target_with_derived_qualifier`
- `cargo test -p kaveon-sql --lib product_merge_supports_bounded_matched_update` — 1 passed, 0 failed on 2026-11-02
- `cargo test -p kaveon-sql --lib product_merge_supports_bounded_literal_matched_update` — 1 passed, 0 failed on 2026-11-03
- `cargo test -p kaveon-sql --lib product_merge_accepts_an_unaliased_target_with_derived_qualifier` — 1 passed, 0 failed on 2026-11-02
- `cargo test -p kaveon-server sql_endpoint_bounded_merge_matched_update_is_cas_checked_and_unmatched_is_noop` — 1 passed, 0 failed on 2026-11-02
- `cargo test -p kaveon-sql --lib product_merge_supports_bounded_not_matched_by_source_delete` — 1 passed, 0 failed on 2026-11-02
- `cargo test -p kaveon-server --bin kaveon-server sql_endpoint_bounded_merge_not_matched_by_source_deletes_owned_rows` — 1 passed, 0 failed on 2026-11-02

The transaction API matched-update test first commits the source-expression
form, then commits a literal-assignment form through a second transaction. It
reads the committed snapshot and verifies the revision advances to `3`, the
literal document is persisted, and the unmatched source key is a no-op. Both
forms stage through the same `ProductDmlCommand::Upsert` path used by the
qualified conflict-upsert tests.
