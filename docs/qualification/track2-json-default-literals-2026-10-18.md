# Track 2 JSON literal `DEFAULT` qualification — 2026-10-18

## Boundary

`JSON_VALUE` and `JSON_QUERY` accept literal `ON EMPTY DEFAULT` and `ON ERROR
DEFAULT` actions. Supported defaults are SQL string, numeric, boolean, and NULL
literals. The parser lowers them to a bounded four-argument evaluator function;
the evaluator distinguishes malformed JSON (`ON ERROR`) from a valid document
with no compatible result (`ON EMPTY`). Arbitrary expressions, typed
`RETURNING` with `DEFAULT`, mixed NULL/ERROR/DEFAULT combinations, and JSON
format/coercion rules remain fail-closed.

## Evidence

- `engine/crates/sql/src/parser.rs`:
  `json_default_literal_lowers_to_bounded_evaluator_function` verifies both
  clause positions and NULL fallback lowering.
- `engine/crates/sql/src/parser.rs`:
  `unsupported_json_default_expression_fails_closed` and
  `json_default_with_returning_remains_rejected` verify the unsupported
  expression/type boundary; a quoted `RETURNING` token remains a literal.
- `engine/crates/exec/src/expr_eval.rs`:
  `json_default_literals_cover_empty_and_error_paths` verifies a present scalar,
  missing path, and malformed document across the two fallback actions.
- Focused validation: `cargo test -p kaveon-sql json_default --quiet` (4 passed)
  and `cargo test -p kaveon-exec json_default_literals_cover_empty_and_error_paths --quiet`
  (1 passed).

## Remaining scope

Typed defaults, arbitrary default expressions, mixed action combinations,
complete JSON path/coercion semantics, and external-client/distributed
qualification remain open.
