# AKS five-minute mixed-workload soak — 2026-10-13

The current Engine image completed a five-minute AKS mixed-workload soak with four concurrent clients. The harness completed 416 exact-result requests across metadata-backed counts and a distributed grouped query. All 416 results matched their baselines; all three workers remained Ready with zero unexpected restarts, and coordinator/worker retained exchange and spill file counts remained zero.

Machine-readable evidence: [engine-aks-current-soak-2026-10-13-300s.json](engine-aks-current-soak-2026-10-13-300s.json)

This is sustained evidence for the declared current workload and image. It is not a multi-hour endurance, upgrade/rollback, or broad capacity claim.
