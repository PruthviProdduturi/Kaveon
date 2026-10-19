# Decimal128 typed join runtime filter

## Scope

Single-key Decimal128 joins now use a typed `i128` hash index for probe
filtering and storage `IN` pushdown. This avoids the generic row-key materialization
path while preserving the declared decimal scale and exact duplicate semantics.

## Evidence

- The local join regression builds `[125, 400, 400]` and probes
  `[100, 125, 400, NULL]`; the typed filter returns exactly the two matching
  probe rows and emits two distinct storage literals.
- The partitioned three-worker-style regression preserves duplicate build
  matches, rejects the nonmatching and NULL probe rows, and releases all query
  memory after completion.

Commands:

```powershell
cargo test -p kaveon-exec decimal128_runtime_filter_uses_typed_index_and_storage_predicate -- --nocapture
cargo test -p kaveon-exec partitioned_join_applies_decimal128_runtime_filter_without_precision_loss -- --nocapture
```

## Boundary

This qualifies local and in-process partitioned execution. Decimal256 remains
on its existing typed path, while remote ADLS/Delta deployed-image timing and
cloud pruning remain separate qualification work.
