# Track 2: `JSON_EXISTS` `ON ERROR` actions

`JSON_EXISTS` now accepts the bounded Trino SQL/JSON error actions:

```sql
JSON_EXISTS(document, path TRUE ON ERROR)
JSON_EXISTS(document, path FALSE ON ERROR)
JSON_EXISTS(document, path UNKNOWN ON ERROR)
```

The parser lowers these clauses to explicit evaluator variants. A malformed
JSON document returns `TRUE`, `FALSE`, or SQL `NULL` according to the selected
action, while a valid document with a missing path still returns `FALSE`.
Duplicate error clauses fail before planning.

Evidence:

- `cargo test -p kaveon-sql json_exists_error --quiet` — 2 passed.
- `cargo test -p kaveon-exec evaluates_json_exists_error_actions_for_malformed_documents --quiet` — 1 passed.
- `cargo test -p kaveon-server --bin kaveon-server distributed_json_exists_error_actions_match_local -- --nocapture` — passed with exact local/distributed results.

The boundary remains intentionally narrow: `PASSING` variables, arbitrary
JSON path filters, and full SQL/JSON error classification for malformed paths
remain outside this qualification. The explicit actions distinguish malformed
documents; syntactically invalid paths still use the existing fail-closed
`FALSE` behavior.
