# Distributed typed JSON coercion qualification

Date: 2026-10-18

The differential sweep now covers typed SQL/JSON extraction for Boolean and
VARCHAR scalar values and VARCHAR materialization of a JSON query result:

```sql
JSON_VALUE(JSON_PARSE('{"enabled":true}'), '$.enabled' RETURNING BOOLEAN)
JSON_VALUE(JSON_PARSE('{"label":"ready"}'), '$.label' RETURNING VARCHAR)
JSON_QUERY(JSON_PARSE('{"items":[1,2]}'), '$.items' RETURNING VARCHAR)
```

Each case matches across dictionary, plain, split, clustered, partitioned,
Hive ordered, and three-worker distributed execution paths. The focused gate is
`the_differential_sweep_matches_across_parquet_encodings_and_execution_paths`
with `KAVEON_DIFF_ONLY` selecting each case.
