# Track 2 JSON `ON EMPTY` / `ON ERROR` bounded actions

Date: 2026-10-18  
Scope: bounded parser and evaluator compatibility for `JSON_VALUE` and `JSON_QUERY`

## Implemented

The SQL parser accepts `ON EMPTY NULL` and `ON ERROR NULL` after the supported
JSON path and optional `RETURNING` clause. It removes those explicit clauses
before planning because the existing evaluator already returns `NULL` for a
missing path, a non-scalar `JSON_VALUE` result, and malformed JSON. This keeps
the accepted form equivalent to the current execution behavior.

It also accepts an all-`ERROR` clause combination (`ON EMPTY ERROR`,
`ON ERROR ERROR`) and lowers it to a strict evaluator path. Missing paths,
malformed documents, non-scalar `JSON_VALUE` results, and non-object/array
`JSON_QUERY` results fail the query with an execution error. Typed `RETURNING`
casts remain supported through the same lowering.

`DEFAULT` actions, mixed NULL/ERROR combinations, and malformed `ON` clauses
fail closed with a parser diagnostic. They are not silently treated as NULL or
ERROR.

## Validation

- `cargo fmt --all -- --check`
- `cargo clippy -p kaveon-sql --all-targets -- -D warnings`
- `cargo test -p kaveon-sql --quiet` — 210 passed
- `node scripts/validate-docs.mjs` — passed
- `git diff --check` — passed

## Remaining scope

`ON EMPTY DEFAULT`, `ON ERROR DEFAULT`, combinations that mix NULL and ERROR,
`FORMAT`, full JSON path/coercion semantics, and broader external-client and
distributed differential qualification remain unsupported.
