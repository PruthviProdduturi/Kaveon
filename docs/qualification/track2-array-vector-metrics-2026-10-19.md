# Track 2 qualification: array vector metrics

Date: 2026-10-19

Kaveon supports Trino's numeric vector functions
`EUCLIDEAN_DISTANCE(array(double), array(double))` and
`DOT_PRODUCT(array(double), array(double))`. Both require equal-length arrays
per row and return `DOUBLE`. NULL arrays or NULL elements produce a NULL row;
unequal cardinalities fail closed instead of silently truncating or padding.

## Evidence

Run from `engine/`:

```powershell
cargo test -p kaveon-exec --lib vector_array_metrics
```

The distributed differential corpus compares both functions against the local
evaluator through the three-worker fragment path, including typed DOUBLE output
and the documented vector values.

External-client and cloud qualification remain part of the broader Track 2
boundary.
