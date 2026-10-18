# Track 1 — exact `COUNT(DISTINCT)` nested-value identity

Date: 2026-10-18  
Scope: local `kaveon-exec` aggregate path

## Change

The exact aggregate key path now canonicalizes unsupported nested Arrow values
through the same type-tagged representation used by the distinct operator.
`COUNT(DISTINCT list/map/row)` therefore compares logical values rather than
Arrow's physical offsets and buffers. The old debug representation could encode
two equal nested values differently when they came from different list slots or
batches.

## Evidence

```text
cargo test -p kaveon-exec count_distinct_nested_values_ignores_arrow_offsets -- --nocapture
test aggregate::tests::count_distinct_nested_values_ignores_arrow_offsets ... ok
test result: 1 passed; 0 failed
```

The regression builds two separate list slots containing `[1, 2]`, followed by
`[2]` and `NULL`; exact `COUNT(DISTINCT value)` returns `2`, ignoring `NULL` and
the differing physical offsets.

## Boundary

This is local aggregate coverage. It does not qualify all nested types through
the deployed exchange path, spill behavior for unbounded distinct state, or an
external pinned-Trino differential run. Those remain open in the parity tracker.
