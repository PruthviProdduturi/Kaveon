# AKS sustained mixed-workload soak — 2026-10-13

The current Engine image completed a 60-second AKS mixed-workload soak with four concurrent clients. The harness issued 88 exact-result requests across metadata-backed counts and a distributed grouped query. All 88 results matched their baselines; all three workers remained Ready with zero unexpected restarts, and coordinator/worker retained exchange and spill file counts remained zero.

Machine-readable evidence: [engine-aks-current-soak-2026-10-13.json](engine-aks-current-soak-2026-10-13.json)

This is a bounded current-workload soak, not a broad capacity or multi-hour production endurance claim.
