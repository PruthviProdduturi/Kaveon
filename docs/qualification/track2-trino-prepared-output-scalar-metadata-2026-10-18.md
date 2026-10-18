# Track 2 qualification: prepared output scalar metadata

Date: 2026-10-18  
Surface: Trino-compatible `DESCRIBE OUTPUT` planning metadata  
Status: qualified by focused regression

## Scope

Prepared output metadata now recognizes common scalar projection expressions:

- arithmetic over integer and floating-point literals reports `bigint` or `double`;
- comparison and predicate expressions report `boolean`;
- string concatenation reports `varchar`;
- `EXTRACT` reports `bigint`.

The regression `trino_describe_output_infers_scalar_expression_metadata` in
`engine/crates/server/src/api.rs` plans this projection and verifies every
column's metadata:

```sql
SELECT 1 + 2 AS total,
       1 < 2 AS is_ordered,
       'a' || 'b' AS label
```

Expected output types are `bigint`, `boolean`, and `varchar`.

## Boundary

This qualifies metadata inference for these scalar expression classes only.
Column-derived type propagation, decimal arithmetic, complex `ARRAY`/`MAP`/
`ROW` signatures, wildcard expansion, and external client release validation
remain separate work. The result does not claim execution support for every
expression accepted by Trino.
