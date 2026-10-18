# Track 1 — Page-index and Bloom storage qualification

## Scope

This qualification covers the local Parquet and object-store reader primitives
behind Track 1 E3. It verifies that a clustered file carries page indexes and
Bloom filters, that equality predicates use Bloom filters before decoding, and
that an offset index leaves unrelated data pages unread when late
materialisation is enabled. It is a storage qualification; it does not claim
the end-to-end `surface = 'Chat'` or cloud latency target is closed.

## Evidence

The focused storage tests passed from `engine/` on source `6ca6359f`:

```text
cargo test -p kaveon-storage row_filter_admits_exactly_the_matching_rows_over_object_storage -- --nocapture
test result: ok. 1 passed; 0 failed; 127 filtered out

cargo test -p kaveon-storage a_clustered_layout_reads_fewer_row_groups_pages_and_bytes_than_the_same_rows_unclustered -- --nocapture
point: row groups 2/2 → 1/8; rows examined 2000000 → 250000;
       bytes selected 23118285 → 1454051; bytes read 23118285 → 783217
range: row groups 2/2 → 1/8; rows examined 2000000 → 250000;
       bytes selected 23118285 → 1453914; bytes read 23118285 → 799369
test result: ok. 1 passed; 0 failed; 127 filtered out
```

The Bloom test also checks local, ADLS-shaped, and generic object-store readers
against the same answers and metrics. Each equality/`IN` probe reads all eight
Bloom filters, prunes row groups that cannot contain the value, and emits
exactly the twelve matching rows. The offset-index comparison proves that the
late-materialised object path reads fewer compressed bytes than the selected
row-group total.

## Boundary

The reader implementation is verified for page-index and Bloom behavior on
deterministic local fixtures. The remaining E3 work is the workload gate:
measure representative dictionary-encoded production-shaped files and qualify
the end-to-end latency target on the current distributed image, including the
cloud snapshot path. Those boundaries remain Partial in the Trino parity
tracker.
