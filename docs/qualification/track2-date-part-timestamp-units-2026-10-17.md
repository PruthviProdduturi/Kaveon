# Track 2 qualification: temporal extraction units

Date: 2026-10-17  
Scope: timestamp values represented in Arrow seconds, milliseconds, and
nanoseconds, Arrow Date64, and Arrow Time32/Time64 values.

`DATE_PART` now normalizes all timestamp units to microseconds before applying
the existing calendar extraction logic. This removes the previous unit-specific
behavior where only microsecond timestamps were accepted. `DATE_DIFF` likewise
normalizes Arrow Time32 and Time64 values for fixed units.

Focused execution coverage evaluates the `SECOND` field against equivalent
second, millisecond, nanosecond, Date64, and Time64 values and verifies the same
result for each representation. It also verifies `DATE_DIFF` across mixed
Time32 and Time64 units, including a 90-second and one-hour difference. The
execution crate passes the warning-clean clippy gate.

Time-zone conversion, interval fields, and broader external-client coverage
remain open in the parity tracker.
