# KaveonDB typed-schema integrity qualification — 2026-10-28

## Scope

This qualification covers the immutable KaveonDB manifest's typed-row schema
integrity contract. It is not a claim that arbitrary lake tables expose SQL
Server or PostgreSQL constraint DDL.

## Qualified behavior

- Typed tables validate a declared primary key and declared unique keys.
- Rows are checked against the declared schema before publication, including
  type bindings and bounded JSON values.
- Primary and unique indexes are rebuilt from the final candidate snapshot and
  validated before the snapshot can become current.
- Product references are resolved against the final snapshot; dangling or
  wrong-kind references are rejected, and referenced product deletion is
  restricted.
- Multi-table changes publish atomically, so a rejected constraint or
  reference leaves the prior snapshot unchanged.
- Index tampering and inconsistent reference indexes fail validation rather
  than being silently repaired.

## Evidence

The catalog suite covers:

- `typed_rows_insert_update_delete_atomically_enforce_keys_and_revisions`
- `typed_row_indexes_round_trip_and_tampering_is_rejected`
- `explicit_typed_schema_accepts_multi_row_atomic_batch_and_rejects_type_drift`
- `unique_indexes_validate_the_final_atomic_snapshot`
- `rejects_dangling_and_wrong_kind_product_references`
- `delete_restricts_referenced_product_records`
- `same_transaction_parent_and_child_resolve_in_final_snapshot`
- `a_reference_index_that_disagrees_with_the_records_is_refused`

General SQL `CREATE TABLE`, `ALTER TABLE`, arbitrary constraint names,
foreign-key DDL, secondary indexes, and lake-table mutation remain outside
this contract and continue to require separate qualification.
