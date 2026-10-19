# Timestamp typed join runtime filter

## Scope

Single-key timestamp joins now use a typed `i64` hash index for runtime
filtering and hash probing. Second, millisecond, microsecond, and nanosecond
timestamps retain their native integer values and declared timezone metadata.
Storage `IN` pushdown emits one exact integer literal per distinct build key.
Changed probe unit or timezone metadata fails closed before reinterpretation.

## Evidence

- The local index regression checks all four units, both unzoned and UTC
  timestamps, exact values `-9007199254740993` and `9007199254740993`, duplicate
  build matches, NULL rejection, distinct storage literals, and rejection of
  changed probe metadata.
- The storage predicate regression compiles the exact `IN` literals against
  every unit and timezone variant. It selects exactly the two matching rows,
  rejects the adjacent `9007199254740992` value and NULL, and preserves native
  integer precision.
- The partitioned three-partition regression preserves the exact three joined
  rows, including the duplicate build match, across all eight type variants.
  Query memory returns to zero after every run.
- The existing local Parquet regression still accepts the timestamp runtime
  filter before decoding and returns exactly the matching row.

From `engine/`:

```powershell
cargo test -p kaveon-exec timestamp_runtime_filter -- --nocapture
cargo test -p kaveon-storage timestamp_runtime_filter -- --nocapture
cargo test -p kaveon-exec join::tests -- --nocapture
cargo test -p kaveon-exec runtime_join_filter_uses_typed_unsigned_and_date_indexes -- --nocapture
cargo clippy -p kaveon-exec -p kaveon-storage --all-targets -- -D warnings
git diff --check
```

Results: 2 timestamp execution tests, 2 timestamp storage tests, 44 join and
semijoin tests, and the existing mixed-type runtime-filter test passed.
Clippy and the whitespace check passed.

## Boundary

This is local and in-process partitioned qualification. It does not qualify
deployed-image timestamp behavior, cross-unit coercion, or cloud pruning.
Representative cloud file/row-group pruning and snapshot qualification remain
open Track 1 gates.
