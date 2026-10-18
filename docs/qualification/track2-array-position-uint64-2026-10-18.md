# Track 2 qualification: `ARRAY_POSITION` over regular `UInt64` lists

Kaveon evaluates `ARRAY_POSITION` over regular Arrow `List<UInt64>` values in
the element's native width. It preserves one-based positions, matches the
full `UInt64` range (including `u64::MAX`), and returns `0` when no element
matches. Null list elements and a nullable needle retain Trino's null-match
behavior.

The evaluator regression is `evaluates_array_position_for_uint64_lists` in
`engine/crates/exec/src/expr_eval.rs`. The distributed differential corpus
includes `uint64_array_position`, which compares the result across the local,
partitioned, and three-worker fragment paths against the pinned Trino
reference.

Validation:

```text
cargo test -p kaveon-exec evaluates_array_position_for_uint64_lists -- --nocapture
KAVEON_DIFF_ONLY=uint64_array_position cargo test -p kaveon-server --bin kaveon-server the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --nocapture
cargo clippy -p kaveon-exec -p kaveon-server --all-targets -- -D warnings
cargo fmt --manifest-path engine/Cargo.toml --all -- --check
```
