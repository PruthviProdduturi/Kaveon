# Qualification: filtered `MERGE` delete-or-insert

Date: 2026-11-04  
Scope: KaveonDB-owned product records

## Contract

The Engine accepts the bounded SQL Server form:

```sql
MERGE INTO product.datasets AS target
USING (VALUES ('delete-me', '{"ignored":true}'),
              ('keep-existing', '{"keep":true}'),
              ('new-row', '{"created":true}')) AS source (id, document_json)
ON target.id = source.id
WHEN MATCHED AND source.id IN ('delete-me') THEN DELETE
WHEN NOT MATCHED THEN
  INSERT (id, document_json) VALUES (source.id, source.document_json);
```

The source is a bounded `VALUES` batch. Existing target rows are deleted only
when their source id is in the literal matched filter. Existing rows outside
the filter are no-ops. Absent source rows still follow the `NOT MATCHED`
insert action. The transaction layer resolves each command against the pinned
snapshot, checks ownership, and stages ordinary revision/CAS `DELETE` or
`CREATE` commands. The existing 1,000-source-row limit remains in force.

`OUTPUT deleted.id, deleted.revision, deleted.document_json` is also
qualified for this form. It returns only selected delete actions, including
the document captured from the pinned pre-delete snapshot; inserted and
excluded no-op source rows do not appear in the deleted projection.

General predicates, `NOT IN`, arbitrary assignments, unbounded source scans,
and filtered `WHEN NOT MATCHED BY SOURCE` combinations remain rejected.

## Evidence

- Parser: `product_merge_supports_filtered_matched_delete_with_insert_fallback`.
- Transaction/API: `sql_endpoint_bounded_merge_filtered_match_deletes_and_inserts`.
- The test verifies that the selected existing row is deleted, the excluded
  existing row is unchanged, and an absent source row is created at revision 1.
- The same API test verifies the deleted identity, revision, and pre-delete
  document in the SQL response.
- SQL library suite: 386 passed; bounded server MERGE suite: 14 passed.
- Focused clippy passes with `-D warnings`.

This is a bounded product-transaction qualification, not a claim of general
lake-table `MERGE` support.
