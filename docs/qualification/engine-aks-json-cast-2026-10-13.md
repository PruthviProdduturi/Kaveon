# AKS JSON cast qualification — 13 October 2026

Image: `kaveonacr.azurecr.io/kaveon-engine@sha256:55b7f72c725c0d2b38a45bd8d8a8ea096f8b82c68a6ceb54b741d0b95fb5b537`
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
