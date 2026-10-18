# Track 1 — dictionary runtime-filter value types

## Scope

Runtime-filter literals now unwrap Arrow dictionary columns to their value
type before construction. Numeric, boolean, temporal, and decimal build keys
can therefore reach the same exact comparison path as plain columns; the
dictionary keys are still evaluated through the dictionary values, avoiding
row-wise text or numeric decoding.

## Evidence

From `engine/`:

```text
cargo test -p kaveon-storage scan_predicate::tests -- --nocapture
test result: ok; 10 passed; 0 failed
```

The focused regression evaluates an `Int64` runtime `IN` filter against an
`Int32`-keyed dictionary column and verifies exact row selection. Existing
decimal, temporal, float, range, composition, and dictionary-text predicate
tests also pass.

## Boundary

This qualifies dictionary value-type coercion in the local storage predicate
compiler. It does not claim a deployed cloud qualification or support for
dictionary physical encodings outside the Arrow value types accepted by the
storage reader.
