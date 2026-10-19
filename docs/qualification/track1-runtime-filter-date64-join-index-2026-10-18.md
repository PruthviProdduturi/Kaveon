# Date64 typed join runtime filter

## Scope

Single-key Date64 joins now use a typed `i64` hash index instead of the
generic row-key materialization path. Storage pushdown retains Date64 values
as exact integer literals, and NULL probe keys remain excluded by SQL equality
semantics.

## Evidence

- The local regression builds two distinct dates with a duplicate and probes a
  nonmatch, both matches, and NULL. The filter emits exactly the two matching
  rows and two storage literals.
- The partitioned runtime-filter regression exercises UInt64, Date32, Date64,
  and timestamp keys through the same three-worker-style join path, preserving
  exact matches and NULL rejection.

Commands:

```powershell
cargo test -p kaveon-exec date64_runtime_filter_uses_typed_index_and_preserves_nulls -- --nocapture
cargo test -p kaveon-exec partitioned_join_uses_typed_unsigned_and_date_indexes -- --nocapture
```

## Boundary

This is local and in-process partitioned evidence. Timestamp unit-specific
runtime-filter specialization and deployed ADLS/Delta qualification remain
separate work.
