# Track 1 — non-finite floating-point runtime-filter safety

## Scope

Runtime filter literals cross the join/storage boundary as `Float64`. NaN and positive or negative infinity now decline source pushdown for both `Float32` and `Float64` probe columns. Finite values retain the existing exact-representation checks, and the exact executor path remains responsible for non-finite comparison semantics.

## Evidence

From `engine/`:

```text
cargo test -p kaveon-storage --lib scan_predicate --quiet
cargo fmt --all -- --check
cargo clippy -p kaveon-storage --all-targets -- -D warnings
```

The focused regression verifies NaN, positive infinity, and negative infinity fail closed for both probe types, while a finite Float64 value remains eligible.

## Boundary

This is local storage predicate evidence. Cloud/object-reader qualification for non-finite values and the broader external-client differential corpus remain open.