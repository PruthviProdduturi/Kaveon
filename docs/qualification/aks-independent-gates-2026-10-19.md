# AKS-independent qualification gates — 2026-10-19

This report records evidence that can be collected without an AKS subscription.
It is deliberately separate from deployed qualification: it does not claim
ADLS fault handling, node loss, cloud topology, or multi-container network
behavior.

## Distributed execution and recovery

The focused in-process distributed gate passed **8/8, 0 failures** at source
commit `f5e91aa5`. It covered:

- distributed join spill refusal with no retained spill files;
- grouped aggregate correctness under spill;
- worker loss between producers and consumers;
- worker loss during producer execution;
- global ordering after grouped aggregation and worker loss;
- queued query cancellation; and
- query-history recovery after coordinator restart.

These tests exercise the same fragment, exchange, retry, cleanup, and journal
contracts used by the local distributed runtime. They are source-level and
in-process; they do not replace a deployed worker or node-drain test.

## Storage and commit path

The storage crate gate passed **154 tests, 0 failures, 2 ignored**. The focused
ADLS commit-path gate passed **12 tests, 0 failures, 1 ignored**. Storage smoke
checks for a 1M-row full scan, projection, and single-row-group read also
completed successfully.

The opt-in Azurite Compose profile is now running alongside the local stack.
The ignored provider test passed **1/1** against Azurite, including the real
Azure-protocol `409 BlobAlreadyExists` and `412 ConditionNotMet` responses:

```text
cargo test -p kaveon-storage azurite -- --ignored --nocapture
test adls_commit::tests::azurite_conditional_commit_matches_the_contract ... ok
```

The remaining container-level work is the full Trino differential run and
failure injection across the networked Compose topology.

## Remaining environment-dependent gates

- Docker/WSL multi-container topology and Trino differential comparison;
- Azurite provider-error, retry, and conditional-write qualification;
- AKS ADLS reads, node loss, deployed worker topology, and cloud endurance;
- broader skew, ordered-streaming, federation, and sustained mixed-workload
  qualification.
