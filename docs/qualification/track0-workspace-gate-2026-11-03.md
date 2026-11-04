# Engine workspace gate — 2026-11-04

## Boundary

This record covers the source-level release gate for the current `dev`
branch after the strict invariant T-SQL `PARSE` qualification in
`289d664e` (and the deterministic ordering correction in `39fe66a3`). It is a
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
- execution: 644 passed, 4 ignored;
- optimizer: 86 passed;
- SQL: 384 passed;
- storage: 158 passed, 3 ignored;
- server differential suite: 714 passed, 0 failed, 0 ignored;
- the 173-case T-SQL distributed differential sweep completed successfully;
- workspace benchmark targets completed successfully.

The corrected qualification query uses a complete ordering key
(`user_id, event_date, actions, country`) so ties cannot be mistaken for a
distributed semantic mismatch. The full run completed with exit code 0.

The strict invariant `PARSE` numeric and temporal differentials were included
in the server suite and passed against the local reference and three-worker
execution.

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

The lint gate passed with `-D warnings`. Repository validators also passed:

```text
python scripts/validate-tsql-compatibility-tracker.py
PASS: 181 unique T-SQL compatibility cases, engine pin 7b509256, tracker count synchronized
node scripts/validate-docs.mjs
Documentation validation passed: 727 Markdown files, 39 routes, 9 SVGs.
```

## Boundary that remains open

This gate does not qualify adaptive join policy, distributed global sort or
remaining window shapes, general lake-table DML, cross-source joins,
external JDBC/ODBC clients, object-store failure endurance, or a deployed
cloud image. Those remain separate qualification gates in the compatibility
plan and parity tracker.
