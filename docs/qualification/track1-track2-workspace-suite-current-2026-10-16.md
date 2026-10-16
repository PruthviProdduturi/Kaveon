# Track 1 and Track 2 workspace qualification — current source

**Date:** 2026-10-16  
**Source:** `origin/dev` at `1b5e0c98` (the workspace gate includes the
correlated array-function, map-key, and map-entry regressions)

The complete Rust workspace test command was run from `engine/`:

```text
cargo test --workspace --quiet
```

The command completed with **1,520 passed, 6 ignored, and 0 failed** across
all workspace test binaries. The long server differential sweep completed
successfully. Component totals were 74 catalog, 212 CLI, 87 core, 375
execution, 68 optimizer, 409 server, 176 SQL, and 125 storage tests.

The release lint commands also passed:

```text
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
```

This is repository-level evidence only. It does not close the remaining Track
1 AKS gates or Track 2 compatibility boundaries; those remain listed in the
parity trackers and require their corresponding live or differential evidence.
