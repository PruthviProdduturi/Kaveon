# Track 2 `ARRAY_APPEND` and `ARRAY_PREPEND` qualification

This records the exact-result differential qualification for the array append
and prepend functions on the pinned Rust toolchain. The cases run through the
node-local planner and the in-process three-worker distributed fragment path
over the dictionary, plain, partitioned, and clustered Parquet layouts.

## Evidence

- Commit: `e3591a88`
- Toolchain: Rust `1.98.1` from `engine/rust-toolchain.toml`
- Differential gate:

  ```powershell
  cd engine
  cargo test -p kaveon-server differential_tests --quiet
  ```

- Result: **41 passed, 0 failed, 0 ignored** (including the append/prepend
  case in `array_scalar_functions`)
- Lint gate:

  ```powershell
  cargo clippy --workspace --all-targets -- -D warnings
  ```

- Result: passed
- Documentation gate:

  ```powershell
  node scripts/validate-docs.mjs
  ```

- Result: passed
- Remote Engine workflow: run `39008654155` completed green through Linux,
  Windows, macOS, CLI workflows, and the dev Release job.

The query uses Trino's array-first signatures:
`ARRAY_APPEND(ARRAY[1, 2], 3)` and `ARRAY_PREPEND(ARRAY[1, 2], 0)`. Exact
results matched across every local and distributed storage/execution path.
