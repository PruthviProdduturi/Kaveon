# Track 2 qualification: temporal `ARRAY_DISTINCT` units

Date: 2026-10-17  
Scope: regular `List` and `LargeList` inputs containing Arrow temporal values.

`ARRAY_DISTINCT` now supports `Time64` microsecond/nanosecond values and
timestamp second/millisecond/nanosecond values. It preserves first-seen order
and removes repeated temporal values, including on the large-list path after
its existing safe offset normalization.

Focused execution coverage verifies duplicate removal for millisecond
timestamps in a regular list and nanosecond times in a large list. The
execution crate passes the warning-clean clippy gate.

Broader temporal interval, time-zone, and external-client qualification remain
open in the parity tracker.
