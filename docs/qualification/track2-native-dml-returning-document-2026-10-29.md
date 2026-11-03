# Product DML `RETURNING document_json` qualification — 2026-10-29

Kaveon’s bounded product transaction facade now returns the staged document
for product `INSERT`, revision-checked `UPDATE`/`DELETE`, and bounded
`ON CONFLICT DO UPDATE` upsert statements. DELETE captures the pre-delete
immutable document from the transaction snapshot, or from transaction-staged
bytes when an earlier write in the same transaction changed the record. The
response is therefore tied to the same revision-checked snapshot as the CAS
write.

`RETURNING *` is also qualified for product records. It expands to the stable
product contract (`id`, `revision`, `document_json`) rather than implying an
arbitrary typed-row or lake-table schema.

The same stable projection is now exercised through the typed-row facade for
bounded conflict-upsert, revision-checked update, and delete. The typed row document is
returned as `document_json`; arbitrary typed-column expansion remains outside
the contract.

The bounded same-kind source-copy `INSERT ... SELECT` form also accepts
`RETURNING *` when narrowed by its literal `id IN (...)` filter; the result uses
the same stable product projection. Unfiltered source-copy returning remains
rejected.

Typed-row DELETE now returns a canonical typed-row document with its table,
owner, revision, columns, and unique-key metadata. Bounded source-copy
`INSERT ... SELECT RETURNING document_json` remains rejected because expansion
does not preserve a per-command returning projection.

Evidence:

- Parser regression: `accepts_identity_and_document_returning_for_product_dml`.
- Transaction API regressions: `sql_endpoint_returns_product_identity_metadata_from_staged_dml`
  covers product INSERT `RETURNING *`, while
  `sql_endpoint_update_and_delete_return_identity_and_revision` covers revision-CAS
  UPDATE `RETURNING *` and verifies the staged document alongside the identity and
  incremented revision. `typed_row_transactions_are_owner_isolated_and_rollback_is_invisible`
  covers typed-row conflict-upsert, update, and delete `RETURNING *`.
- `sql_endpoint_commits_bounded_product_table_insert_select` covers filtered
  source-copy `RETURNING *` and verifies the copied document.
- Delete capture is read-before-stage and validates owner plus expected
  revision before returning the document.
- The focused SQL parser and transaction API tests pass.
- Strict workspace Clippy and formatting pass after the implementation.

This does not expand the contract to arbitrary typed lake-table DML,
multi-row source scans, or general `RETURNING` expressions.
