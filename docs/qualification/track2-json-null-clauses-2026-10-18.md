# Track 2 JSON `ON EMPTY NULL` / `ON ERROR NULL`

Date: 2026-10-18  
Scope: bounded parser compatibility for `JSON_VALUE` and `JSON_QUERY`

## Implemented

The SQL parser accepts `ON EMPTY NULL` and `ON ERROR NULL` after the supported
JSON path and optional `RETURNING` clause. It removes those explicit clauses
before planning because the existing evaluator already returns `NULL` for a
missing path, a non-scalar `JSON_VALUE` result, and malformed JSON. This keeps
the accepted form equivalent to the current execution behavior.

Unsupported actions (`DEFAULT` and `ERROR`) and malformed `ON` clauses fail
closed with a parser diagnostic. They are not silently treated as `NULL`.

## Validation

- `cargo fmt --all -- --check`
- `cargo clippy -p kaveon-sql --all-targets -- -D warnings`
- `cargo test -p kaveon-sql --quiet` — 209 passed
- `node scripts/validate-docs.mjs` — passed
- `git diff --check` — passed

## Remaining scope

`ON EMPTY DEFAULT`, `ON EMPTY ERROR`, `ON ERROR DEFAULT`, `ON ERROR ERROR`,
`FORMAT`, full JSON path/coercion semantics, and broader external-client and
distributed differential qualification remain unsupported.
