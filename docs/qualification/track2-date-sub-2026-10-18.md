# Track 2 qualification: `DATE_SUB`

`DATE_SUB(unit, amount, value)` now lowers to the Engine's existing typed date
operators, matching Trino's calendar subtraction surface for day, week, month,
quarter, year, hour, minute, second, and millisecond units. Literal offsets are
folded with calendar month-end clamping, while expression offsets retain their
per-row arithmetic. Overflow during unit scaling or sign inversion is rejected
with an actionable SQL error.

## Focused evidence

- `cargo fmt --all -- --check` — pass.
- `cargo test -p kaveon-sql date_sub_lowers_to_negative_typed_offsets --quiet` — pass.
- `cargo test -p kaveon-server differential_tests::date_sub_trino_units --quiet` — pass.
- `cargo clippy -p kaveon-sql --all-targets -- -D warnings` — pass.

The full differential sweep was not used as the gate for this bounded change;
the existing suite exceeded two minutes without completing in this workspace.
