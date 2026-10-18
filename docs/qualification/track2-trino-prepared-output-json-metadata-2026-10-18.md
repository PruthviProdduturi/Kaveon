# Trino prepared output metadata qualification

Date: 2026-10-18

## Scope

This qualification covers the Trino `DESCRIBE OUTPUT` metadata inference path for expressions that produce JSON and bigint values. The adapter must expose stable Trino type names before executing the prepared query.

Projection:

```sql
SELECT JSON_PARSE('{"x": 1}') AS document,
       CARDINALITY(ARRAY[1, 2]) AS size
```

## Result

The metadata adapter returns `document → json` and `size → bigint`, retaining both projection aliases. The regression is `trino_describe_output_infers_json_and_cardinality_metadata` in `engine/crates/server/src/api.rs`.

## Boundary

This qualifies bounded prepared output metadata inference for JSON parsing and cardinality. It does not claim complete type inference for every function, wildcard expansion, external Trino client release coverage, or cloud qualification.
