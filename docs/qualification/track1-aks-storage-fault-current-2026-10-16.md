# Track 1 AKS: current-image object-store fault recovery

## Result

The current AKS image passed the scoped ADLS/object-store fault qualification.
The harness temporarily blocked public object-store egress for `kaveon-worker-0`
while a distributed scan was running, then removed the policy in cleanup.

All checks passed:

- the post-fault result matched the exact baseline (`COUNT(*) = 504000000`);
- a stage retry was observed;
- no exchange or spill files remained before or after the fault;
- credentials were not recorded and no subscription-level policy was changed.

The run used catalog snapshot
`sha256:652742525737d79eb579cdef4a8386b6fa6c9f3feedc60b197bd3406ab73d1f3` and
three workers. The raw harness report is retained in
`track1-aks-storage-fault-current-2026-10-16.json`.

## Boundary

This refreshes the bounded exchange/object-store recovery evidence for Track 1.
Cross-region storage, physical disk exhaustion, and broader endurance remain
open.
