# AKS active-query node-drain qualification — 2026-10-13

The current Engine image passed the `current` OpenSource workload gate with an actual node drain. During a running 504M-row distributed scan, worker 0's node was drained and the worker was evicted. The coordinator observed the worker loss, created a retry attempt, returned the exact baseline result, and restored a catalog-compatible three-worker cluster. The post-recovery exact probe and bounded concurrent pressure tail also passed.

Machine-readable evidence: [engine-aks-fault-pressure-2026-10-13-node-drain.json](engine-aks-fault-pressure-2026-10-13-node-drain.json)

This closes active-query worker recovery during node drain for the declared workload and image. Coordinator exchange-spool cleanup, coordinator active-query restart recovery, sustained soak, and cloud-storage fault injection remain separate gates.
