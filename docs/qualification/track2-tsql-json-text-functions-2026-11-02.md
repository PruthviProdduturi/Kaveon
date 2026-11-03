# Track 2 qualification — native SQL Server JSON text functions

## Scope

This qualification adds the native SQL Server/Fabric spelling of
`JSON_VALUE` and `JSON_QUERY` to the bounded T-SQL read contract. The input is
text JSON and the path is a literal SQL/JSON path. `JSON_VALUE` returns a scalar
text value and returns `NULL` for a container. `JSON_QUERY` returns canonical
JSON text for an object or array result.

The contract is deliberately separate from Trino's `JSON_PARSE` and typed
`RETURNING` forms. It does not claim complete SQL Server JSON path,
collation, or coercion parity.

## Exact qualification

The server differential test
`distributed_tsql_json_text_functions_match_local` evaluates the same query
through the local planner and the three-worker fragment path:

```sql
SELECT
  JSON_VALUE('{"customer":{"id":42},"items":[1,2]}', '$.customer.id'),
  JSON_VALUE('{"customer":{"id":42},"items":[1,2]}', '$.customer'),
  JSON_QUERY('{"customer":{"id":42},"items":[1,2]}', '$.customer'),
  JSON_QUERY('{"customer":{"id":42},"items":[1,2]}', '$.items')
```

Expected rows are repeated identically for all three input rows:

```text
42 | NULL | {"id":42} | [1,2]
```

The focused result was exact: local and three-worker canonical rows matched,
the scalar/container boundary held, and the query memory pool returned to zero
after both executions. The planner regression
`accepts_tsql_json_value_and_json_query_text_forms` confirms the native
function spelling remains accepted as a T-SQL read shape.

## Boundary

This is a bounded Engine read qualification. Dynamic paths, complete SQL Server
error/coercion modes, collation behavior, and external-client/cloud
qualification remain outside this record.
