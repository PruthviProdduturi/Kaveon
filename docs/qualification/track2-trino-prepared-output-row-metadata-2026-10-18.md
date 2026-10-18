# Track 2 qualification: prepared output ROW metadata

Date: 2026-10-18  
Surface: Trino-compatible `DESCRIBE OUTPUT` planning metadata  
Status: qualified by focused regression

## Scope

Prepared output metadata now reports field signatures for literal `ROW(...)`
expressions. Each field is inferred recursively from the existing expression
metadata rules and emitted in Trino's `row(...)` type signature.

The regression `trino_describe_output_infers_row_metadata` in
`engine/crates/server/src/api.rs` verifies:

```sql
SELECT ROW(1, 'ready') AS state,
       ROW(2, 3.5) AS measurements
```

Expected output types are `row(bigint,varchar)` and
`row(bigint,decimal(2,1))`.

## Boundary

This qualifies literal ROW construction in prepared output metadata. Named
ROW fields, column-derived ROW signatures, nested complex coercion, wildcard
expansion, and external client release validation remain separate work. The
result does not claim execution support for every ROW expression accepted by
Trino.
