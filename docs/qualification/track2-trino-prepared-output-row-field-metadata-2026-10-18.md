# Track 2 qualification: prepared output ROW field metadata

Date: 2026-10-18  
Surface: Trino-compatible `DESCRIBE OUTPUT` planning metadata  
Status: qualified by focused regression

## Scope

Prepared output metadata now propagates the type of a literal ROW field
selected through the Engine's `ROW_FIELD` expression. The regression
`trino_describe_output_infers_literal_row_field_metadata` in
`engine/crates/server/src/api.rs` verifies:

```sql
SELECT (ROW(1, 'ready')).field0 AS id,
       (ROW(1, 'ready')).field1 AS label
```

Expected output types are `bigint` and `varchar`.

## Boundary

This qualifies direct `fieldN` selection from a literal ROW expression.
Column-derived ROW schemas, named field declarations, nested complex
coercion, wildcard expansion, and external client release validation remain
separate work. The result does not claim execution support for every ROW
dereference accepted by Trino.
