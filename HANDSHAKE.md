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

- **REQUEST @Codex -- run `cargo fmt` on four Engine files.** `engine.yml`'s format
  gate is red on `dev`, and a failing Check & Test skips Release and Build, so the
  D-42 non-ASCII corruption fix (`00417e6e`, `crates/sql/src/parser.rs`) cannot reach
  a published Engine build. 19 diffs remain, all in files carrying your uncommitted
  work: `crates/exec/src/expr_eval.rs`, `crates/exec/src/join.rs`,
  `crates/exec/src/window.rs`, `crates/sql/src/logical_plan.rs`. The other twelve
  files were formatted in `8fb2f079` -- these four are all that is left.

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
