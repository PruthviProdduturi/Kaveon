# Workspace release test gate — 2026-11-22

## Command

```text
cargo test --workspace --release --locked -- --test-threads=1
```

Executed from `engine/` on the pinned Rust `1.98.1` toolchain. The command
completed with exit code `0`. The workspace test listing contained 2,417 test
entries across catalog, CLI, core, execution, optimizer, server, SQL, and
storage. Every non-ignored test passed; the run reported zero failures.

The server portion included the complete 721-test release suite, including the
distributed differential corpus, exchange/recovery, spill, cancellation,
catalog, transaction, API, and lifecycle tests. Tests requiring an external
Azurite service or explicitly marked benchmark-only remain ignored by their
declared contract; they are not represented as cloud qualification.

This is local release evidence only. It does not close the AKS/ADLS sustained
soak, cloud identity rotation, backup/restore, or upgrade/rollback gates.
