# Track 2 qualification: `JSON_SIZE` container, scalar, and NULL results

Date: 2026-10-18

## Contract

`JSON_SIZE(document, path)` returns the member count for arrays and objects,
returns zero for a matched scalar, and preserves the JSON null result as zero
according to the Trino JSON function contract. A missing or malformed path
continues to produce NULL.

## Focused evidence

The distributed differential corpus includes
`json_size_container_scalar_null` across the Parquet layouts and three-worker
fragment path. It compares array, scalar, and JSON-null members at the same
path with Trino.

Run from `engine/`:

```powershell
$env:KAVEON_DIFF_ONLY = "json_size_container_scalar_null"
cargo test -p kaveon-server --bin kaveon-server the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --nocapture
```
