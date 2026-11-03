# Kaveon collaboration contract

This file is a stable handoff contract, not a work log. Do not append session notes,
progress updates, or agent activity here.

## Source of truth

- Use Git commits and pull requests for implementation history.
- Use focused tests and files under `docs/qualification/` for reproducible evidence.
- Use `STATUS.md` for the current product capability ledger.
- Use `docs/engineering/trino-parity-tracker.md` for the current Trino parity matrix
  and acceptance gates.
- Use `engine/DISTRIBUTED_EXECUTION_STATUS.md` for the reviewed distributed-engine
  capability snapshot.
- Use `docs/engineering/decision-log.md` for durable architectural decisions.

## Open requests

Live cross-owner asks, one line each, **removed when done**. This is a worklist, not
a log: nothing dated, nothing historical, no progress updates. It exists because
`CLAUDE.md` prescribes `REQUEST @Codex` / `REQUEST @Claude` and the freeze left that
mechanism without a venue, so hand-offs were going into commit messages where the
other engineer does not see them.

There are two open cross-owner requests:

- **REQUEST @Codex:** the row serialiser's `DataType` match in `engine/crates/server/src/api.rs` has no `Utf8View` arm, so at `:14884` and `:14979` every `Utf8View` value falls to `_ => format!("{:?}", arr.slice(row, 1))` and leaves `/v1/statement` as `StringViewArray\n[\n  "AFG",\n]` instead of `AFG`; `Utf8` is unaffected, so `CAST(col AS VARCHAR)` is the current workaround. Every string column in the showcase Parquet is `Utf8View` and `engine_bridge.execute` posts to the same route, so each Engine-backed chart with a text dimension renders debug text as its category labels and a map keyed on those values joins nothing.

- **REQUEST @Claude:** start an Engine-adjacent lane under explicit architect authorization: review the current Rust Engine Track 1/2 handoff and take one bounded, testable engine qualification item in parallel with Codex. Keep API/Studio work isolated, do not touch `docs/reference/engine-sql-compatibility.md`, and return the commit, focused evidence, validation results, and remaining boundary. This request records the ownership exception for this lane.

## Collaboration rules

- Read the ownership rules in `CLAUDE.md` before crossing an ownership boundary.
- Keep changes scoped to the files owned by the current task.
- Record implementation details in the commit message and tests, not in a running
  Markdown diary.
- Update a capability document only when a meaningful implementation or qualification
  boundary changes; replace its current matrix entry instead of appending a session log.
- Never place credentials, tokens, connection strings, or user data in the repository.

The former append-only coordination history is preserved at
[`docs/archive/handshake-history.md`](docs/archive/handshake-history.md) for historical
reference only. It is frozen and must not receive new entries.
