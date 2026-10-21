# Kaveon release readiness — 2026-10-20

This is a current-state snapshot, not a claim of cloud release completion.

## Passed locally

- Server target: 611/611 tests, including the full differential sweep.
- Catalog target: 89 passed, 1 ignored.
- Storage target: 154 passed, 3 ignored.
- Workspace formatting and strict Clippy: passed.
- Distributed reliability subset: 9/9 passed.
- Azurite conditional-commit/provider tests: 2/2 passed.
- Trino differential: 12/12 exact cases at 500k rows with a release binary.
- Docker two-worker differential: 12/12 exact cases at 500k rows with the
  rebuilt current-source image and distributed stages.
- Docker result cache: 10/10 warm hits; restart miss and repopulation verified.

## Open qualification gates

- AKS node-loss, rolling upgrade/rollback, and sustained cloud endurance.
- Live ADLS data-plane reads, writes, conditional commits, and fault injection.
- Larger skew/disk-exhaustion and multi-coordinator failover qualification.
- Broader SQL/DML and external-client compatibility.
- High-cardinality aggregate and grouped-join performance optimization.

## Current Azure precondition

`kaveon-aks` is deallocated with provisioning state `Failed`, and authenticated
access to `kaveonlake` returns `AccountIsDisabled`. Run
`scripts/check-aks-cloud-precondition.ps1` before attempting cloud tests; it
fails closed until both resources are usable.

- Workspace release build: `cargo build --workspace --release --locked` passed on
  the current integration checkout (2m04s).
- Docker Compose configuration validation: `docker compose config --quiet` passed.
- Server differential sweep rerun independently: **1 passed, 0 failed** in
  593.12s (`the_differential_sweep_matches_across_parquet_encodings_and_execution_paths`).
