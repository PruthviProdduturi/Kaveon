# Track 2 qualification: temporal `ARRAY_SORT` units

Date: 2026-10-17  
Scope: regular `List` and `LargeList` inputs containing Arrow temporal values.

`ARRAY_SORT` now accepts `Time64` microsecond/nanosecond values and timestamp
second/millisecond/nanosecond values. It uses the existing null-last ordering
and preserves the temporal Arrow type in the returned list for both list
offset widths.

Focused execution coverage verifies ascending millisecond timestamp ordering
in a regular list and nanosecond time ordering in a large list. The execution
crate also passes its warning-clean clippy gate:

```text
cargo test -p kaveon-exec array_sort --quiet
cargo clippy -p kaveon-exec --all-targets -- -D warnings
```

Broader temporal interval, time-zone, and distributed external-client coverage
remains open in the parity tracker.
