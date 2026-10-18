# Track 1 global TopN: empty candidate reduction

## Scope

Selective distributed scans often return empty candidate batches from some
workers. The coordinator's typed and sorted TopN paths should do work only for
workers that contribute rows.

## Result

The TopN merge filters empty candidate batches once before typed key evaluation
and the sorted k-way comparator construction. This keeps the result exact
while reducing key evaluation and comparator state in sparse worker merges;
schema validation remains applied to every input batch.

Focused qualification:

```text
cargo test -p kaveon-exec global_top_n_ignores_empty_worker_candidates_without_changing_order
1 passed; 0 failed
cargo clippy -p kaveon-exec --all-targets -- -D warnings
passed
```

The regression compares the same two-row result with and without empty worker
candidates across a multi-column ordering.

## Boundary

This reduces coordinator work for sparse candidate sets. Positive-limit global
ordering still has a coordinator merge stage; removing that bottleneck requires
a streaming or tree-based exchange protocol.
