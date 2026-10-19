# Track 2: `JSON_EXISTS` malformed-path error actions

Kaveon's bounded `JSON_EXISTS` evaluator now distinguishes an invalid runtime SQL/JSON path from a valid path whose member is absent. The former is an `ON ERROR` condition, so `TRUE ON ERROR`, `UNKNOWN ON ERROR`, and the default false behavior return `TRUE`, `NULL`, and `FALSE` respectively. A valid missing member remains `FALSE`, including with `TRUE ON ERROR`.

The qualified statement is:

```sql
SELECT
  JSON_EXISTS('{"present":1}', '$.present[' TRUE ON ERROR),
  JSON_EXISTS('{"present":1}', '$.present[' UNKNOWN ON ERROR),
  JSON_EXISTS('{"present":1}', '$.missing' TRUE ON ERROR)
FROM events
LIMIT 1
```

The local evaluator and the three-worker in-process distributed path both return `TRUE`, `NULL`, `FALSE` exactly. The regression tests are `evaluates_json_exists_error_actions_for_malformed_paths` in `engine/crates/exec/src/expr_eval.rs` and `distributed_json_exists_malformed_path_error_actions_match_local` in `engine/crates/server/src/differential_tests.rs`.

This is a bounded runtime-path improvement. Full SQL/JSON path grammar, coercion, and external-client/cloud differential coverage remain open.
