# Track 1 — narrow numeric window SUM

The window evaluator now widens signed `TINYINT`, `SMALLINT`, `INTEGER`, and
`BIGINT` `SUM` inputs through an exact `i128` accumulator and returns `BIGINT`
(`Int64`). `REAL`/`Float32` inputs are accepted by the existing `Float64`
window aggregate path. This removes the previous worker failure for narrow
window inputs and avoids converting signed integer sums through `f64`.

The focused local test covers `TINYINT`, `SMALLINT`, and `REAL`, including
negative values, NULL peers, cumulative default `RANGE` frames, and exact
`BIGINT` output. The server differential uses a partitioned, ordered
`SUM(CAST(actions AS TINYINT))` with a bounded `ROWS` frame and compares the
local result with the three-worker exchange result.

## Evidence

```text
cargo test -p kaveon-exec window_sum_widens_tinyint_and_real_without_float_input_loss --quiet
1 passed, 0 failed

cargo test -p kaveon-server --bin kaveon-server distributed_partitioned_window_sum_tinyint_widens_exactly --quiet
1 passed, 0 failed
```

This qualifies the signed narrow-input shape on the local and in-process
three-worker topology. It does not qualify arbitrary cloud deployment or
parallel execution of globally ordered windows.
