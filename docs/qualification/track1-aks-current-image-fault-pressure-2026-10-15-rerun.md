# AKS current-image fault and pressure rerun — 2026-10-15

The immutable Engine image
`kaveonacr.azurecr.io/kaveon-engine@sha256:0ea4244c392267f5d821935cb63f4c2a251c30be2f44a33554b340e1eb63a1f6`
passed the complete fault and pressure harness on the declared three-worker AKS
cluster. The rerun began with zero retained coordinator exchange files and zero
worker spill files.

Command:

```text
python scripts/qualify-aks-fault-pressure.py \
  --context kaveon-aks --namespace kaveon --profile current \
  --concurrency 2 --concurrent-rounds 2 --pressure-rounds 2 \
  --output track1-aks-current-image-fault-pressure-2026-10-15-rerun.json
```

All seven checks passed:

- two-way concurrent exact results;
- active-query cancellation and cleanup;
- forced worker loss with an observed retry and exact recovery;
- three catalog-compatible workers after replacement;
- bounded grouped pressure with exact results and no unexpected restart or file growth;
- zero retained exchange or spill files before and after the run; and
- an exact post-recovery probe.

The machine-readable report is
`track1-aks-current-image-fault-pressure-2026-10-15-rerun.json`. This closes the
precondition gap in the earlier near-pass report. Larger high-cardinality joins,
disk-exhaustion behavior, and sustained mixed-workload endurance remain open
qualification gates.
