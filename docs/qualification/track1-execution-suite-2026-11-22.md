# Execution crate qualification — 2026-11-22

## Result

**PASS (serial isolation run).** The complete execution library suite passed
with one test thread: **645 passed, 0 failed, 4 ignored**. The ignored tests
are explicitly benchmark-only or require an external emulator; they are not
silently treated as passing.

```powershell
cargo test -p kaveon-exec --lib -- --test-threads=1
```

The suite covers vectorized aggregates, exact distinct state, Arrow exchange,
joins and runtime filters, spill and bounded-memory behavior, partitioned
execution, top-N, windows, set operations, and cancellation/recovery paths.

## Scheduling note

A parallel `cargo test --workspace --all-targets --exclude kaveon-server`
invocation exposed one timing-sensitive pressure fixture
(`hybrid_merge_spills_for_a_source_thread_the_budget_refuses`); it passed in
isolation and in this complete serial run. This is recorded as a test-harness
stability issue, not suppressed as an expected failure. The fixture should be
made deterministic before using an all-target parallel run as the release
receipt.

No cloud resources were provisioned or accessed.

The exact non-server workspace command used by the release workflow also
completed successfully with one test thread:

```powershell
cargo test --workspace --exclude kaveon-server -- --test-threads=1
```

This covers the catalog, core, SQL, execution, optimizer, storage, and CLI
targets plus their doc tests. The server binary remains a separate serial
step in `.github/workflows/engine.yml` because its process-global query store
and admission fixtures require that isolation for a deterministic gate.
