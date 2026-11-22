# Bracketed DISTINCT identifier runtime check — 2026-11-22

The Studio-generated SQL shape that previously failed with an `unknown scalar
function: ARRAY` worker error was reproduced against the current two-worker
Docker stack:

```sql
SELECT DISTINCT [year]
FROM climate_energy.energy_annual;
```

The current coordinator completed the statement successfully in 767 ms and
returned six integer rows (`2020` through `2025`, with source order preserved
by the distributed result contract). The response schema was `year: Int64`.

The parser and logical-plan regressions are covered by
`bracketed_identifiers_are_not_parsed_as_arrays`,
`studio_bracketed_identifier_lowers_to_a_column`, and
`bracketed_distinct_identifier_is_not_lowered_as_array`. The server result
serializer also has a UTF-8-view regression, preventing text dimensions from
falling back to Arrow debug output.

This check qualifies the local Docker image only; it does not substitute for
the cloud rollout gates.
