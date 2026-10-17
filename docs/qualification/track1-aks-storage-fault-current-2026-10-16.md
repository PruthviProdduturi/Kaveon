# Track 1 AKS storage-fault recovery gate — 2026-10-16

This bounded gate injects one scoped ADLS egress fault on `kaveon-worker-0`
while allowing cluster and DNS traffic. The harness removes the policy in a
`finally` block and never records credentials.

## Environment

- Context: `kaveon-aks`, namespace `kaveon`
- Catalog snapshot: `sha256:652742525737d79eb579cdef4a8386b6fa6c9f3feedc60b197bd3406ab73d1f3`
- Query: `SELECT COUNT(*) FROM kaveon_events_enriched WHERE actions >= 0`
- Baseline result: `504000000`
- Baseline result SHA-256: `17b44689d4c8fb0c665d8a42992b6e0ff9d15a1b3822afd043ad1a9d58b287c5`

## Result

| Check | Result |
| --- | --- |
| Exact result after the injected storage fault | Pass |
| Stage retry observed | Pass |
| No retained coordinator exchange files | Pass |
| No retained worker spill files | Pass |
| Fault policy removed | Pass |

The baseline completed in 26.7 seconds and the fault-injected retry completed
in 91.5 seconds. Both responses had the same result hash and two stages with
four tasks. The raw report is retained only in the local qualification
workspace as `tmp/track1-aks-storage-fault-current.json`.

This qualifies the declared single-worker object-store fault and exact retry
cleanup. It does not close cross-region storage, repeated-fault, disk
exhaustion, or sustained mixed-workload gates.
