# Typed-row conflict-ignore qualification — 2026-10-28

## Contract

`kaveon.product.typed_rows` accepts revisioned create, update, and delete
commands for KaveonDB-owned typed rows. `INSERT ... ON CONFLICT DO NOTHING`
is replay-safe when the incoming row has the same table, primary key,
revision-1 payload, and owner as the existing row. A conflicting payload is
rejected; arbitrary typed-row conflict updates are not part of this contract.

## Evidence

- Parser test: `typed_row_insert_supports_bounded_conflict_ignore`.
- Transaction API test: `typed_row_transactions_are_owner_isolated_and_rollback_is_invisible`.
- The API test creates `app.users/u-1`, replays the identical conflict-ignore
  insert, and verifies the committed revision remains `1`.
- The same test verifies owner isolation, revision-checked update, delete, and
  rollback visibility.

## Boundary

This is a bounded KaveonDB transaction contract, not general SQL-table DML.
Arbitrary typed schemas, conflict-update assignments, lake-table writes,
`MERGE`, constraints, and named conflict targets remain open and continue to
fail closed.

