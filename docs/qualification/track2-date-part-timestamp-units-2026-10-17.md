# Track 2 qualification: `DATE_PART` timestamp units

Date: 2026-10-17  
Scope: timestamp values represented in Arrow seconds, milliseconds, and
nanoseconds and Arrow Date64.

`DATE_PART` now normalizes all timestamp units to microseconds before applying
the existing calendar extraction logic. This removes the previous unit-specific
behavior where only microsecond timestamps were accepted.

Focused execution coverage evaluates the `SECOND` field against equivalent
second, millisecond, nanosecond, and Date64 values and verifies the same
result for each representation. The execution crate passes the warning-clean
clippy gate.

Time-zone conversion, interval fields, and broader external-client coverage
remain open in the parity tracker.
