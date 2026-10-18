# Track 1 — Byte-aware inner join costing

## Scope

When both inner-join inputs expose exact stored-byte statistics, the optimizer now chooses the narrower serialized relation as the build side before falling back to row counts. This avoids selecting a very wide low-row-count relation for build or broadcast solely because it has fewer rows. If either byte estimate is unavailable or equal, the existing row-count policy remains in force.

## Evidence

```text
cargo test -p kaveon-optim --lib
result: 79 passed, 0 failed
cargo clippy -p kaveon-optim --all-targets -- -D warnings
cargo fmt --package kaveon-optim -- --check
```

The focused regression `chooses_the_narrower_inner_build_when_row_counts_disagree` verifies that a 10-row, 512 MiB relation does not replace a 10,000-row, 8 MiB relation as the build side, and that the resulting plan remains partitioned rather than broadcasting an oversized input.

## Boundary

This is a bounded inner equi-join build-side cost improvement. It does not estimate join-key skew or implement general multi-join cost-based enumeration; live AKS qualification remains open.
