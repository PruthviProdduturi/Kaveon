# Track 1 — Join cost upper-bound qualification

## Scope

Filtered relation statistics must remain conservative for join distribution. A small non-empty relation must not round down to zero rows or bytes, because that can make a build appear free and select an unsafe broadcast plan.

## Evidence

```text
cargo test -p kaveon-optim --lib statistics::tests::filtered_estimates_never_round_a_nonempty_relation_to_zero --quiet
cargo clippy -p kaveon-optim --all-targets -- -D warnings
cargo fmt --all -- --check
```

Selectivity scaling now uses a finite ceiling for non-zero inputs. Exact zero selectivity and empty source statistics remain zero; positive estimates are never understated by floating-point rounding.

## Boundary

This closes the local cost-model rounding edge. It does not replace source statistics with runtime feedback or qualify broadcast decisions on AKS under changing cardinalities.
