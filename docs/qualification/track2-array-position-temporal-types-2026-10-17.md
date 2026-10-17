# Track 2 qualification: temporal `ARRAY_POSITION` units

Date: 2026-10-17  
Scope: regular `List` and `LargeList` inputs containing Arrow temporal values.

`ARRAY_POSITION` now accepts `Time64` microsecond/nanosecond values and
timestamp second/millisecond/nanosecond values on the typed regular-list path.
The large-list path continues to use the nested comparator path, which keeps
the same one-based position and zero-for-missing contract.

Focused execution coverage verifies a millisecond timestamp in a regular list
and a nanosecond time in a large list, both returning the correct one-based
position. The execution crate passes its warning-clean clippy gate.

Broader temporal interval, time-zone, and external-client qualification remain
open in the parity tracker.
