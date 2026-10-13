# AKS fault and pressure qualification — 13 October 2026

This artifact records a live pod-loss, cancellation, recovery, and bounded-pressure
run against the `kaveon-aks` cluster in namespace `kaveon`. The machine-readable
report is [engine-aks-fault-pressure-2026-10-13.json](engine-aks-fault-pressure-2026-10-13.json).

The run used the current deployed image (`67ad56af`, catalog snapshot
`sha256:38c8c04bb4df47e0737556c7316254beedf8d858591bf22ac374c68a01188f3e`)
and the declared OpenSource workload. It did not change subscription policies
and recorded no credential material. The cluster started with three
catalog-compatible workers and finished with three catalog-compatible workers
after replacing the intentionally terminated worker.

## Result

All checks in the report passed:

- four-way concurrent exact-result rounds completed with typed result hashes;
- an active query was canceled through the API, returned the expected conflict
  response while cancellation was in progress, reached `CANCELED`, and left zero
  coordinator exchange or worker spill files;
- a running worker pod was terminated after task execution began; the retry
  completed with the exact baseline result (`COUNT(*) = 504,000,000`), and a
  replacement worker joined with the same catalog snapshot;
- two rounds of four-way grouped-aggregation pressure completed with every
  accepted request returning HTTP 200/`FINISHED`, no unexpected restarts, and
  sampled engine memory within pod limits;
- the post-recovery probe returned its exact expected result;
- no pre-existing retained exchange or spill files were present.

Admission refusals are recorded separately by the runner and are not counted as
successful statements. The final run completed after the bounded admission retry
policy; this demonstrates safe backpressure, not unlimited capacity.

## SLO interpretation

This is positive evidence for the Track 1 distributed-execution SLO, covering
worker-loss retry and recovery, cancellation cleanup, exact results under
concurrency, and bounded pressure for this image and workload. It is not the
complete release SLO artifact: the five-minute mixed-workload success-rate window,
coordinator-loss behavior, sustained network and cloud-storage fault injection,
longer endurance, and upgrade/rollback evidence remain outstanding. The tracker
therefore remains **Partial** and the release gate stays fail-closed.

## Reproduction

```powershell
python scripts/qualify-aks-fault-pressure.py `
  --context kaveon-aks `
  --namespace kaveon `
  --profile current `
  --failure-mode pod `
  --concurrency 4 `
  --concurrent-rounds 2 `
  --pressure-rounds 2 `
  --output tmp/qualification-67ad56af.json
```

The report was captured at `2026-10-13T23:02:43Z` and completed at
`2026-10-13T23:09:44Z`. The report's `pods_before` and `pods_after` sections
bind the evidence to the deployed image digests and catalog snapshot. The
archived JSON is the complete credential-safe runner output; it records the
worker replacement, exact result hashes, request states, and resource samples.
