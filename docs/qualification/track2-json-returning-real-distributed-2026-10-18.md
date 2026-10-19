# Track 2 qualification: JSON_VALUE RETURNING REAL

## Scope

This qualification covers the bounded SQL/JSON coercion surface for
`JSON_VALUE(... RETURNING REAL)`. Trino `REAL` is IEEE-754 single precision;
Kaveon represents it as Arrow `Float32`, so fractional values are rounded at
the target type boundary. JSON `null` remains SQL `NULL`.

The qualification does not claim complete SQL/JSON coercion, path, or error
semantics. Those remain partial in the parity tracker.

## Evidence

The parser test `parses_real_as_single_precision` verifies that `REAL` lowers
to `CastTarget::Float32`, while `DOUBLE` remains `Float64`. The evaluator test
`evaluates_real_cast_as_float32_with_null_preservation` verifies the Arrow
result type, Float32 rounding, and null validity.

The server differential test
`distributed_json_value_returning_real_matches_local` runs a typed
`JSON_VALUE` over a fractional JSON number and JSON null, then compares local
execution with the three-worker fragment path. Both paths produce three rows
of `1.2345679, NULL`, and the query memory pool returns to zero after each
path.

Validation on 2026-10-18:

```text
cargo test -p kaveon-sql parses_real_as_single_precision -q                         PASS
cargo test -p kaveon-exec evaluates_real_cast_as_float32_with_null_preservation -q  PASS
cargo test -p kaveon-server distributed_json_value_returning_real_matches_local      PASS
```

