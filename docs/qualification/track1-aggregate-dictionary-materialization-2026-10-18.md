# Track 1 UTF-8 dictionary aggregate materialization — 2026-10-18

## Boundary

Grouped UTF-8 batches already use a per-batch code table so repeated rows do
not allocate a new string key. This change extends that boundary to Arrow
dictionary-encoded UTF-8 columns: dictionary entries that are not referenced
by the current batch remain a null sentinel in the code table, and therefore
do not allocate an `Arc<str>` or become an empty output group. Referenced
entries keep their logical text, including dictionary null values, and row
lookup remains an integer code operation.

This is an allocation reduction for the worker-side grouped aggregate. It does
not claim a new storage index, global dictionary reuse across batches, or a
universal high-cardinality latency improvement.

## Evidence

Local exact coverage:

```text
cargo test -p kaveon-exec string_group_keys_are_interned_per_batch -- --nocapture
cargo test -p kaveon-exec dictionary_utf8_group_keys_and_extrema_use_logical_values -- --nocapture
```

The tests pass and cover shared plain UTF-8 keys, dictionary keys, unused
dictionary entries, logical nulls, and UTF-8 MIN/MAX output.

Distributed exact coverage:

```text
cargo test -p kaveon-server distributed_utf8_grouped_aggregate_matches_local -- --nocapture
```

The three-worker fragment result matches the local result exactly and both
query memory pools release all reservations.

## Remaining boundary

High-cardinality UTF-8 grouping still materializes one retained key per
distinct logical group and still performs a final distributed merge. Broader
dictionary reuse, spill/endurance qualification, and larger cloud topologies
remain open in the parity tracker.
