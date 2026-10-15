# Track 1 AKS current-image fault and pressure qualification — 2026-10-15

The current Engine image
`kaveonacr.azurecr.io/kaveon-engine@sha256:215184a5bc538369bf8bc16fe66b3beb1c891326e9263e23c6864e6ccaef88f4`
was exercised on the three-worker AKS topology with the reduced but complete
fault/pressure harness:

```text
python scripts/qualify-aks-fault-pressure.py \
  --context kaveon-aks --namespace kaveon --profile current \
  --failure-mode pod --concurrency 2 \
  --concurrent-rounds 1 --pressure-rounds 1
```

The run passed all seven checks:

- two-way concurrent exact results;
- active-query cancellation with HTTP 204 and zero retained exchange/spill
  files;
- forced worker loss, an observed retry attempt, and an exact 504,000,000-row
  result;
- three catalog-compatible workers after replacement and an exact recovery
  probe;
- bounded grouped pressure with exact six-region reconciliation;
- no unexpected restarts or retained-file growth; and
- sampled Engine memory remained within pod limits.

The raw report is retained in
`track1-aks-current-image-fault-pressure-2026-10-15.json`.

A separate graceful node-drain attempt is retained privately in the run
artifacts. It restored the worker and exact result but did not expose a retry
attempt in the query record, so it is not counted as a retry qualification.
Object-store service fault injection, orphan-spool recovery after an external
storage failure, and longer endurance remain open.
