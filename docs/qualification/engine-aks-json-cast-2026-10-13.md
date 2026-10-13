# AKS JSON cast qualification — 13 October 2026

Image: `kaveonacr.azurecr.io/kaveon-engine@sha256:071794eb6e41e8d01d2c5b5daabb032697f8d2659fef08fa72f74014ad479773`
Topology: `kaveon-aks`, namespace `kaveon`, coordinator and three workers.

The deployed Engine returned HTTP 200 and `FINISHED` for all three cases:

```sql
SELECT CAST('{"x":1}' AS JSON) AS j
FROM OpenSource.public.kaveon_events_dashboard LIMIT 1;

SELECT CAST('{"x":1}' AS JSONB) AS j
FROM OpenSource.public.kaveon_events_dashboard LIMIT 1;

SELECT JSON_EXTRACT(CAST('{"x":1}' AS JSON), '$.x') AS x
FROM OpenSource.public.kaveon_events_dashboard LIMIT 1;
```

The first two results have type `Utf8` and value `{"x":1}`; the extraction
returns type `Utf8` and value `1`. This is the declared Kaveon representation:
JSON and JSONB casts validate and preserve JSON text for the existing JSON
functions. Binary JSON storage and full Trino JSON coercion semantics remain
outside this slice.

The same deployment rejected `CAST('not-json' AS JSON)` with the explicit
`invalid JSON value at row 0` execution error, while
`TRY_CAST('not-json' AS JSON)` returned a NULL value.
