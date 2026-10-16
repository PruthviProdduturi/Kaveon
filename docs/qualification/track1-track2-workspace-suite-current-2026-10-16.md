# Track 1 and Track 2 workspace qualification — current source

**Date:** 2026-10-16  
**Source:** `origin/dev` at `5c118f3b` (the workspace gate includes the
correlated array-function, map-key, map-entry, per-row map-concat, and named
window regressions)

The complete Rust workspace test command was run from `engine/`:

```text
cargo test --workspace --all-targets
```

The command completed with **1,531 listed tests, 0 failed** across all
workspace test binaries; the run also reported the repository's six
intentional/benchmark ignores. The long server differential sweep completed
successfully. Component totals were 74 catalog, 212 CLI, 87 core, 377
execution, 68 optimizer, 409 server, 178 SQL, and 125 storage tests.

The release lint commands also passed:

```text
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
```

This is repository-level evidence only. It does not close the remaining Track
1 AKS gates or Track 2 compatibility boundaries; those remain listed in the
parity trackers and require their corresponding live or differential evidence.
