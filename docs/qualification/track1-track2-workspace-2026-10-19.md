# Track 1/2 workspace qualification — 2026-10-19

This qualification covers pushed source commit `6d2c6c7c`, including the
empty-fragment `MAP_ZIP_WITH` schema fix and its focused regression.

## Validation

The complete workspace test gate passed with no failures:

| Suite | Result |
| --- | ---: |
| SQL | 259 passed, 0 failed |
| Exec | 581 passed, 0 failed, 4 ignored |
| Server | 611 passed, 0 failed; full differential included |
| Storage | 154 passed, 0 failed, 2 ignored |
| Other workspace crates | passed |

The complete server suite, including the local/distributed differential, took
533.98 seconds. `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo fmt --all -- --check`, `git diff --check`, and documentation validation
also passed. Documentation validation reported 550 Markdown files, 39 routes,
and 9 SVGs.

## Scope

This proves source-level correctness for the declared local and in-process
distributed gates. It does not close the parity tracker's remaining cloud
qualification boundaries: AKS/object-store endurance, broader topologies,
ordered streaming, full connector federation, and the remaining unsupported
SQL shapes remain open as recorded in the tracker.
