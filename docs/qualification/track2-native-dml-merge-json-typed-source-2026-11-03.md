# Typed source JSON assignment in bounded `MERGE`

## Status

Qualified for the bounded KaveonDB product-record `MERGE` contract. This is a
syntax qualification for the existing revision/CAS write path, not general
typed lake-table DML or arbitrary JSON expression support.

## Contract

A matched product-record update may assign the direct source document column
through an explicit JSON cast:

```sql
MERGE INTO product.datasets AS target
USING (VALUES ('ds-1', '{"name":"Orders"}')) AS source (id, document_json)
  ON target.id = source.id
WHEN MATCHED THEN
  UPDATE SET target.document_json = CAST(source.document_json AS JSON)
WHEN NOT MATCHED THEN
  INSERT (id, document_json) VALUES (source.id, source.document_json)
```

The parser accepts only a direct `source.document_json` operand wrapped in the
`JSON` cast. It lowers to the same `ProductDmlCommand::Upsert` as the direct
source assignment, so owner isolation, revision CAS, pinned-snapshot
visibility, atomic commit, and replay behavior are unchanged.

## Evidence

- `parser::tests::product_merge_accepts_a_typed_source_document_assignment`
- `transaction_api::tests::sql_endpoint_bounded_merge_batch_updates_and_inserts_atomically`
- `cargo test -p kaveon-sql product_merge_accepts_a_typed_source_document_assignment` — 1 passed, 0 failed on 2026-11-03
- `cargo test -p kaveon-server --bin kaveon-server sql_endpoint_bounded_merge_batch_updates_and_inserts_atomically` — 1 passed, 0 failed on 2026-11-03

## Deliberate boundaries

Arbitrary JSON expressions, casts of other columns, source queries or joins,
cross-kind targets, unbounded batches, and general lake-table `MERGE` remain
outside the qualified contract.
