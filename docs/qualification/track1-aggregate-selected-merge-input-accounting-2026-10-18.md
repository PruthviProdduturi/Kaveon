# Track 1 distributed aggregate: selected merge input accounting

Date: 2026-10-18

## Change

The partitioned final aggregate merge receives batches from the bounded
parallel input queue. The queue already holds the source batch reservation
until every merge worker releases its view. The columnar selected-row merge
used to reserve `batch.get_array_memory_size()` again before consuming a
worker's selected row indices. On a wide grouped-state batch, each worker
could therefore be charged for the complete batch even when it owned only a
small fraction of the rows. That duplicate charge created avoidable spill
pressure and could make a bounded query fail before the merge state itself
reached its limit.

`IncrementalAggregateMerger::push_selected_batch` now treats the input as a
borrowed batch. It charges only selected-row scratch, growth, and newly
created group state. The caller retains the queue reservation for the full
batch for the duration of the call. The ordinary `push_batch` path keeps its
own transient input reservation for callers that do not have a queued input
handle.

## Evidence

The focused regression constructs 20,000 encoded grouped states in one wide
batch, selects one row, and runs the selected merge under a 64 KiB operator
budget. The batch itself is larger than that budget. The merge succeeds,
creates exactly one group, and releases all query memory. This is the
bounded-memory shape used by the distributed final merge; it would fail when
the selected path charged the whole batch a second time.

Commands:

```text
cargo fmt --all
cargo test -p kaveon-exec selected_merge_does_not_double_charge_a_wide_input_batch -- --nocapture
cargo test -p kaveon-exec incremental_aggregate::tests -- --nocapture
```

Observed: focused test passed; incremental aggregate tests passed (10 passed,
1 ignored). The distributed final-merge spill and source-pressure tests remain
the broader regression suite and are run with the server test target.

## Boundary

This removes a duplicate in-memory charge from the selected-row final merge.
It does not change spill-file limits, repartition policy, or the physical disk
endurance boundary. Larger skew, disk exhaustion, broader topologies, and
cloud endurance remain open in the parity tracker.
