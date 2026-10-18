# Track 2 qualification: JSON array contains with Decimal and NULL needles

Date: 2026-10-18

## Contract

`JSON_ARRAY_CONTAINS` compares a JSON numeric element with a typed DECIMAL
needle without converting the needle through a binary floating-point type. A
NULL needle preserves Trino's NULL result rather than treating NULL as a JSON
value to search for.

## Focused evidence

The distributed differential corpus includes
`json_array_contains_decimal_and_null` across the Parquet layouts and the
three-worker fragment path. It checks a `DECIMAL(10,2)` needle against
`[1.25, 2.50, null]` and a NULL needle against `[1, null, 3]`, comparing exact
results with Trino.

Run from `engine/`:

```powershell
$env:KAVEON_DIFF_ONLY = "json_array_contains_decimal_and_null"
cargo test -p kaveon-server --bin kaveon-server the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --nocapture
```
