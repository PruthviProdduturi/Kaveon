# Track 2 JSON default malformed-path qualification

## Scope

This qualification covers the bounded SQL/JSON `JSON_VALUE` default form:

```sql
JSON_VALUE(document, path
  ON EMPTY DEFAULT 'empty-default'
  ON ERROR DEFAULT 'error-default')
```

An invalid SQL/JSON path is an error, while a syntactically valid path that
does not select a member is an empty result. The evaluator therefore routes
the first case to the `ON ERROR` default and the second to the `ON EMPTY`
default. This keeps the two SQL/JSON conditions distinct instead of treating
both as a missing value.

## Evidence

- Local executor regression:
  `json_default_literals_route_malformed_paths_to_error_default` in
  `engine/crates/exec/src/expr_eval.rs`.
- Three-worker differential:
  `distributed_json_value_default_on_malformed_path_matches_local` in
  `engine/crates/server/src/differential_tests.rs`.
- Input path `$.customer.id[` returns `error-default`; valid missing path
  `$.customer.id` returns `empty-default`.
- The local and three-worker result sets are identical for three input rows;
  the memory pool returns to zero after execution.

## Boundary

This qualifies the existing bounded path grammar and literal default actions.
Full SQL/JSON path semantics and non-literal default expressions remain
outside the current Track 2 contract.
