# Seven- and eight-way bounded join reordering

## Scope

The statistics optimizer now enumerates connected left-deep orders for inner
equi-join chains containing four through eight base relations. The rewrite
requires exact current row counts, schemas, and byte estimates for every leaf;
outer joins, `USING`, residual predicates, duplicate aliases, missing facts,
and bushy graphs remain on the conservative path. The generated physical plan
is wrapped in an explicit projection so SQL output order remains the written
relation/column order.

## Reproduction

From `engine/`:

```powershell
cargo test -p kaveon-optim --all-targets statistics::tests::reorders_seven_and_eight_way_chains_without_changing_output_order
```

Expected result:

```text
test result: ok. 1 passed; 0 failed
```

The test covers both seven- and eight-relation chains, exact source statistics,
connected key edges, bounded search, and the restored projection column order.
The full optimizer target suite also passed with 86 tests and zero failures.

## Boundary

This is local optimizer evidence only. It does not qualify nine-or-more
relations, bushy plans, selectivity-aware cardinality estimation, or AKS
performance under skew. Those remain explicit tracker items.
