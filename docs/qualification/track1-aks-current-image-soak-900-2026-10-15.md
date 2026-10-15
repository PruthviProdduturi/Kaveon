# Track 1: current-image ADLS soak (900 seconds)

**Date:** 2026-10-15  
**Environment:** AKS `kaveon-aks`, namespace `kaveon`, West US 2  
**Image:** `kaveonacr.azurecr.io/kaveon-engine@sha256:215184a5bc538369bf8bc16fe66b3beb1c891326e9263e23c6864e6ccaef88f4`  
**Command:**

```powershell
python scripts/soak-aks-current.py `
  --context kaveon-aks `
  --namespace kaveon `
  --duration-seconds 900 `
  --concurrency 2 `
  --output tmp/aks-current-soak-900-2026-10-15.json
```

## Result

**PASS.** The run completed 1,180 of 1,180 requests with exact results and no
failures. All workers remained ready, there were no unexpected restarts, and
the coordinator and workers had zero retained exchange or spill-file growth.

| Check | Result |
|---|---:|
| Duration | 900 seconds |
| Concurrency | 2 |
| Requests completed | 1,180 |
| Exact results | 1,180 |
| Failures | 0 |
| Unexpected restart delta | 0 |
| Retained exchange/spill files after run | 0 |

The raw report is retained beside this document as
`track1-aks-current-image-soak-900-2026-10-15.json`.

## Boundary

This strengthens current-image endurance evidence for Track 1 item 2 and
Track 1 item 16. It does not close the exchange gate: authenticated external
object-store fault injection, orphan-spool recovery under that fault, and the
remaining distributed join, aggregate, window, ordering, connector, and
upgrade qualifications remain open in the tracker.
