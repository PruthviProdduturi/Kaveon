# T-SQL `DATETIMEOFFSETFROMPARTS` qualification — 2026-10-29

Kaveon accepts a bounded `DATETIMEOFFSETFROMPARTS` form and emits a
timezone-bearing microsecond timestamp normalized to the UTC instant. The
offset is applied during planning; the original textual offset is not retained
as display metadata.

Qualified contract:

- Ten arguments are required.
- Precision is a literal in `0..6`; precision 7 is rejected because the
  Engine's qualified temporal storage is microsecond-based.
- Timezone hour/minute offsets are literals, limited to ±14:00, with
  minute-range and sign-consistency validation.
- Calendar and clock validation, NULL propagation, and arithmetic use the
  existing typed date/timestamp operators.

Evidence:

- Planner regression: `lowers_tsql_datetimeoffsetfromparts_to_utc_timestamp`.
- Differential case: `tsql_datetimeoffsetfromparts` in the local/three-worker
  sweep.
- Versioned corpus case: `datetimeoffsetfromparts`.

This does not claim preservation of the source offset label, precision-7
100-nanosecond storage, or general timezone/DATETIMEOFFSET parity.
