# Track 2 qualification: `MAP_ZIP_WITH`

## Scope

The Engine supports the bounded Trino form
`MAP_ZIP_WITH(left_map, right_map, (key, left_value, right_value) -> value)`.
The lambda is serialized in the logical plan and evaluated with typed key and
value bindings. Keys are emitted in left-map order, followed by keys present
only in the right map. A missing value is a typed NULL; a NULL input map
produces a NULL output map for that row. The result map preserves row
boundaries and rejects mismatched key types, duplicate lambda parameters, and
non-scalar lambda results.

This is a bounded scalar higher-order function. It does not claim complete
Trino map-function coverage or external/cloud qualification.

## Evidence

- SQL parser: `cargo test -p kaveon-sql --lib`
- Evaluator: `cargo test -p kaveon-exec --lib`
- Differential corpus: `map_zip_with_lambda` in
  `engine/crates/server/src/differential_tests.rs`, which compares local and
  distributed execution across the corpus layouts.

The focused evaluator regression verifies union order and typed NULL padding:
`MAP({'a': 1, 'b': 2}, {'b': 10, 'c': 20})` with a three-parameter lambda
returns keys `a,b,c` and values `1,12,20`.

## Boundary

Nested/lateral map-producing expressions, key coercion between different map
key types, and external-client/cloud qualification remain open.
