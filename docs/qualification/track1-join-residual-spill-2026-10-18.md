# Track 1 — Spilled distributed join residual qualification

## Scope

Partitioned inner joins must preserve non-equi residual predicates after the build side spills. Hash partitioning may reduce candidate pairs, but it must not turn the residual into an in-memory-only behavior.

## Evidence

```text
cargo test -p kaveon-exec --lib partitioned::tests::a_spilled_partitioned_inner_join_preserves_residual_matches --quiet
cargo fmt --all -- --check
cargo clippy -p kaveon-exec --all-targets -- -D warnings
```

The regression compares a spilled partitioned inner join against the qualified in-memory join for 6,000 probe rows and 4,000 build rows, with a supplier inequality residual. It requires exact output pairs, observed spill bytes, and zero retained memory or spill bytes after drain.

## Boundary

This is local bounded spill evidence. AKS high-cardinality residual joins and sustained mixed-workload pressure remain open.
