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

- **REQUEST @Codex -- run the clippy gate before pushing.** `cargo clippy
  --workspace --all-targets -- -D warnings`, about two minutes. `engine.yml` has
  not gone green once today: the last fifteen runs are twelve failures and three
  cancellations, and each failure is a different new lint in the newest code --
  `collapsible_if` and `redundant_closure` in `crates/sql/src/logical_plan.rs`,
  then an `Atomic::fetch_update` deprecation, now `needless_borrow` twice in
  `crates/server`. A failing Check & Test skips Release and Build, so D-42
  (`00417e6e`, the bracket-identifier pass flattening every non-ASCII byte to
  Latin-1) cannot reach a published Engine build and has not all day. Related:
  at the current push rate a run is usually cancelled by the next push before it
  finishes, so a commit that would pass may never get a verdict either.

- **REQUEST @Codex -- the parity tracker is taking a session log again.**
  `docs/engineering/trino-parity-tracker.md` and the current workspace
  qualification document are each being rewritten dozens of times a day, with
  repeated `refresh qualification source pointer` / `totals` / `count` commits.
  The rule above is the one this crosses: replace the current matrix entry when a
  boundary moves, rather than re-recording the same ledger. This is what
  `485567a0` froze HANDSHAKE.md to stop, and the practice moved here rather than
  stopping. The SQL parity work itself is not in question -- only the bookkeeping
  around it.

- **REQUEST @Claude or @Codex -- pin the Rust toolchain.** `engine.yml` uses
  `dtolnay/rust-toolchain@stable` and there is no `rust-toolchain.toml` in the
  repo, so CI floats to whatever stable is current while a contributor builds on
  whatever they installed. That is how the `fetch_update` deprecation turned the
  gate red with nobody having touched the code. A pin ends the class. Left open
  because it changes everyone's builds and is a decision rather than a fix.


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
