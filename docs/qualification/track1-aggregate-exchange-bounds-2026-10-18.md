# Track 1 aggregate exchange bounds qualification

## Scope

The columnar grouped-aggregate final merge consumes two binary Arrow columns:
encoded group keys and compact accumulator states. The merge now validates the
requested row count against both columns before parsing or indexing them. A
malformed exchange envelope therefore returns a bounded execution error instead
of indexing past a column and panicking the worker.

## Evidence

- `cargo fmt --all`: passed
- `cargo test -p kaveon-exec --lib encoded_batch_merge_rejects_states_of_another_layout_or_malformed_bytes --quiet`: 1 passed, 0 failed
- `cargo clippy -p kaveon-exec --lib -- -D warnings`: passed
- `git diff --check`: passed

The regression covers a short state column and a short key column, in addition
to the existing malformed-state, layout, and trailing-byte cases. This is a
bounded exchange-input safety qualification; it does not close the larger
distributed aggregate skew, spill-exhaustion, topology, or latency gates.
