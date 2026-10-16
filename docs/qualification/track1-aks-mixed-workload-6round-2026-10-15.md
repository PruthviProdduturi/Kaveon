# AKS five-minute mixed-workload endurance — 2026-10-15

The current three-worker Engine ran six concurrent rounds of four workloads:
full-table aggregation, a bounded high-cardinality join, a partitioned window,
and global ordering over the 504M-row table. The six rounds took 56.266s,
55.484s, 64.125s, 56.625s, 55.391s, and 62.609s (350.584s total).

All 24 requests finished successfully and matched their sequential baselines.
All three workers participated in every round. Six Kubernetes resource samples
were retained; peak sampled worker memory was 678.4 MiB, 878.0 MiB, and 662.0
MiB respectively, with no unexpected restart. No worker spill files remained
after the run and no request failed.

The complete raw report is retained in
`track1-aks-mixed-workload-6round-2026-10-15.json`. This qualifies the declared
five-minute bounded mixed-workload gate. Longer endurance, node-loss during the
soak, disk exhaustion, and broader topologies remain open.
