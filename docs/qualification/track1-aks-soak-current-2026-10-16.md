# Track 1 AKS mixed-workload soak — 2026-10-16

This bounded soak used the current `kaveon-aks` deployment with four
concurrent clients for 120 seconds. It is an endurance snapshot, not a claim
of the longer 900-second or physical disk-exhaustion gates.

## Result

| Measure | Result |
| --- | ---: |
| Requests completed | 168 |
| Exact results | 168 |
| Failures | 0 |
| Unexpected worker restarts | 0 |
| Workers remaining Ready | Yes |
| Coordinator exchange files before / after | 3 / 3 |
| Worker spill files before / after | 0 / 0 |

The four-client workload mixed counts, grouped aggregates, and small catalog
queries. Every response matched its deterministic reference result. The
catalog snapshot was `sha256:652742525737d79eb579cdef4a8386b6fa6c9f3feedc60b197bd3406ab73d1f3`.
The raw report remains local at `tmp/track1-aks-soak-current.json`.

Longer endurance, physical disk exhaustion, node loss during the soak, and
broader topologies remain open qualification boundaries.
