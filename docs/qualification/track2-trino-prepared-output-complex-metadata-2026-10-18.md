# Track 2 qualification: prepared output complex metadata

Date: 2026-10-18  
Surface: Trino-compatible `DESCRIBE OUTPUT` planning metadata  
Status: qualified by focused regression

## Scope

Prepared output metadata now reports signatures for the literal complex
expressions already represented by the SQL binder:

- `ARRAY[...]` reports `array(element_type)`;
- `MAP(ARRAY[...], ARRAY[...])` reports `map(key_type,value_type)`.

The regression `trino_describe_output_infers_array_and_map_metadata` in
`engine/crates/server/src/api.rs` plans this projection and verifies the
metadata returned for both columns:

```sql
SELECT ARRAY[1, 2] AS ids,
       MAP(ARRAY['a'], ARRAY[1]) AS flags
```

Expected output types are `array(bigint)` and `map(varchar,bigint)`.

## Boundary

This qualifies literal array/map signatures for prepared output metadata.
Complex column-derived signatures, ROW signatures, heterogeneous common-type
coercion, wildcard expansion, and external client release validation remain
separate work. The result does not claim execution support for every complex
type accepted by Trino.
