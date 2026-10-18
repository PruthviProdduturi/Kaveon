# Track 2 qualification: prepared complex metadata over the Trino route

Date: 2026-10-18  
Surface: Trino-compatible prepare and `DESCRIBE OUTPUT` route  
Status: qualified by HTTP route regression

## Scope

The external Trino-compatible route now exposes the complex output signature
for a prepared ROW projection. The regression
`trino_sql_prepare_describe_output_exposes_complex_metadata` in
`engine/crates/server/src/api.rs` prepares and describes:

```sql
SELECT ROW(1, 'ready') AS state FROM numbers LIMIT 1
```

The response is HTTP 200 and contains `row(bigint,varchar)` in the standard
`DESCRIBE OUTPUT` data row. The regression also checks the standard Trino
metadata columns and the `typeSignature.rawType` value for the type column.

## Boundary

This validates route shaping and ownership-scoped prepared metadata for one
literal ROW signature. It does not qualify external client release versions,
column-derived complex schemas, wildcard expansion, or all complex types.
