# Product DML `RETURNING document_json` qualification — 2026-10-29

Kaveon’s bounded product transaction facade now returns the staged document
for product `INSERT`, revision-checked `UPDATE`/`DELETE`, and bounded
`ON CONFLICT DO UPDATE` upsert statements. DELETE captures the pre-delete
immutable document from the transaction snapshot, or from transaction-staged
bytes when an earlier write in the same transaction changed the record. The
response is therefore tied to the same revision-checked snapshot as the CAS
write.

Typed-row DELETE and `INSERT ... SELECT RETURNING document_json` remain
rejected explicitly. Typed rows have a separate row contract, and bounded
source-copy expansion does not preserve a per-command returning projection.

Evidence:

- Parser regression: `accepts_identity_and_document_returning_for_product_dml`.
- Transaction API regression: `sql_endpoint_returns_product_identity_metadata_from_staged_dml`.
- Delete capture is read-before-stage and validates owner plus expected
  revision before returning the document.
- The focused SQL parser and transaction API tests pass.
- Strict workspace Clippy and formatting pass after the implementation.

This does not expand the contract to arbitrary typed lake-table DML,
multi-row source scans, or general `RETURNING` expressions.
