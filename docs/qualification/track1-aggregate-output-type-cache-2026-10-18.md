# Track 1 — aggregate output-type cache qualification

## Scope

`PartitionedHashAggregate` now resolves exchanged group-key types and
aggregate state output types once when the operator is constructed. Streaming
partial rounds and spill retries reuse those immutable vectors instead of
re-walking the input schema and aggregate expressions for every emitted batch.
The cache changes planning overhead only; aggregate state encoding, memory
admission, spill behavior, and output values remain unchanged.

## Evidence

From `engine/`:

```text
cargo test -p kaveon-exec partitioned::tests:: -- --nocapture
test result: ok; 49 passed; 0 failed; 1 ignored

cargo check -p kaveon-exec
Finished `dev` profile
```

The partitioned executor corpus covers adaptive partials, grouped and global
aggregates, dictionary keys, memory refusal/replay, spill cleanup, and exact
multi-batch merges. The cached vectors are shared by both the regular and
partial aggregate paths used by those tests.

## Boundary

This is a bounded CPU/allocation reduction for repeated partial rounds. It is
not a claim of vectorized hash aggregation or a measured wall-clock win on the
ClickBench corpus; high-cardinality aggregate hashing and exchange volume
remain separate performance work.
