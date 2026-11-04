# Engine workspace gate — 2026-11-03

## Boundary

This record covers the source-level release gate for the current `dev`
branch after the bounded final-merge memory-admission change. It is a
correctness and build gate, not a claim of cloud deployment qualification or
complete Trino/Fabric/PostgreSQL parity.

## Evidence

The following command completed from `engine/`:

```text
cargo test --workspace --all-targets
```

Observed results:

- catalog: 93 passed, 4 ignored;
- CLI/server and related workspace targets: all reported tests passed;
- execution: 636 passed, 4 ignored;
- optimizer: 86 passed;
- SQL: 382 passed;
- storage: 158 passed, 3 ignored;
- server differential suite: 708 passed, 0 failed, 0 ignored;
- the 173-case T-SQL distributed differential sweep completed successfully;
- workspace benchmark targets completed successfully.

The release-pressure benchmark was also run independently:

```text
cargo test --release -p kaveon-exec \
  final_merge::tests::final_merge_under_pressure -- --ignored --nocapture
```

All 18 tight/roomy partitioned, broadcast, and source-reserving rounds
completed without a memory-limit failure. The full workspace lint gate also
passed:

```text
cargo clippy --workspace --all-targets -- -D warnings
```

## Boundary that remains open

This gate does not qualify adaptive join policy, distributed global sort or
remaining window shapes, general lake-table DML, cross-source joins,
external JDBC/ODBC clients, object-store failure endurance, or a deployed
cloud image. Those remain separate qualification gates in the compatibility
plan and parity tracker.
