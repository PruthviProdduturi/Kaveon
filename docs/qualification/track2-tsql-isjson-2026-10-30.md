# Bounded T-SQL `ISJSON` qualification — 2026-10-30

## Scope

Kaveon qualifies the SQL Server/Fabric `ISJSON(text[, type_constraint])`
predicate over UTF-8, large UTF-8, and dictionary-backed text columns. It
returns the SQL Server-style integer result `1` for valid/matching JSON, `0`
for invalid, non-text, or non-matching input, and `NULL` for a NULL text value.
The bounded type constraints are `VALUE`, `ARRAY`, `OBJECT`, and `SCALAR`; no
locale or compatibility-level behavior is implied.

## Evidence

- `engine/crates/sql/src/logical_plan.rs` canonicalizes `ISJSON` to `IS_JSON`.
- `engine/crates/exec/src/expr_eval.rs` evaluates the predicate vectorized
  with `serde_json` validation and SQL NULL propagation.
- `expr_eval::tests::isjson_returns_sql_server_style_validity` covers valid,
  invalid, and NULL rows exactly.
- The authoritative corpus includes `isjson` with a
  `pending_distributed_qualification` status; this record does not overclaim
  a worker differential that has not been run yet.

## Boundary

Other `ISJSON` type-constraint values and broader SQL/JSON rules are not part
of this qualification and must continue to fail closed.
