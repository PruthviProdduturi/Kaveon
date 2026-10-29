# Product DML `RETURNING document_json` qualification — 2026-10-29

Kaveon’s bounded product transaction facade now returns the staged document
for product `INSERT`, revision-checked `UPDATE`, and bounded `ON CONFLICT DO
UPDATE` upsert statements. The response is derived from the command staged in
the transaction, so it cannot read a later snapshot or expose another owner’s
record.

`DELETE ... RETURNING document_json` remains rejected explicitly: the staged
delete removes the record and does not retain an old document payload. This is
intentional rather than a fabricated NULL or a post-commit read.

Evidence:

- Parser regression: `accepts_identity_and_document_returning_for_product_dml`.
- Transaction API regression: `sql_endpoint_returns_product_identity_metadata_from_staged_dml`.
- The focused SQL parser and transaction API tests pass.
- Strict workspace Clippy and formatting pass after the implementation.

This does not expand the contract to arbitrary typed lake-table DML,
multi-row source scans, or general `RETURNING` expressions.
