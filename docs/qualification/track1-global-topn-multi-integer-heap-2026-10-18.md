# Global TopN multi-key integer heap qualification

## Scope and result

The sorted, non-null multi-key `Int64`/`UInt64` specialization now keeps one
worker head in a min-heap. Its head selection previously scanned every worker
for each selected row, requiring O(K × P) comparisons for K selected rows and
P worker batches. Heap construction is O(P), and head selection is
O(K × log P). The heap holds at most P row references. Equal ordering keys
retain the previous source-batch order, including rows within each batch.

This bound applies to head selection. Key evaluation, sortedness checks, and
payload materialization retain their existing costs. This is a bounded local
optimization of the existing final merge, not a parallel global result stream
or deployed pressure qualification.

## Evidence

Commands ran from `engine/` on 2026-10-18:

```text
cargo test -p kaveon-exec topn::tests --quiet
28 passed; 0 failed

cargo test -p kaveon-server distributed_global_topn_multi_integer_heap_matches_local -- --nocapture
1 passed; 0 failed

cargo clippy -p kaveon-exec -p kaveon-server --all-targets -- -D warnings

git diff --check
```

The executor regressions call the typed specialization directly and compare
all output columns against Arrow `lexsort_to_indices` over 33 uneven worker
batches. They cover all four combinations of ascending/descending directions,
signed extrema, exact unsigned values near `u64::MAX`, primary-key ties,
worker exhaustion, LIMIT 1, and limits equal to or larger than all input rows.
A separate 17-worker fixture checks distinct payloads under equal complete
ordering keys and a limit cutting through that peer set.

The partitioned regression writes 528 rows into 33 Parquet files, then executes
the same four direction combinations with `LIMIT 73 OFFSET 19` through the
node-local planner and the existing fragment/exchange executor with 3, 17, and
40 workers. All twelve distributed outputs equal the local output exactly,
including payloads and row order, and every run releases its query memory
reservations. Forty workers also exercise tasks with no assigned files.

## Remaining gate

Track 1 item 6 remains open for parallel ordered output and sustained pressure
evidence without a coordinator bottleneck. No network, cloud, or throughput
claim is made by this in-process qualification.
