# Track 1 AKS fault and cancellation rerun

**Date:** 2026-10-14  
**Cluster:** `kaveon-aks` / `kaveon`  
**Image:** `kaveonacr.azurecr.io/kaveon-engine@sha256:c997febb6d14040bd9f26d21d1f96c90f33a6c5faf7a728cf2ba350e8181f06f`

The first run exposed a qualification-harness race: cancellation reused the
worker-loss query and could finish between the RUNNING probe and DELETE. The
harness now uses the grouped exchange query for cancellation, preserving the
worker-loss query for the loss test. The updated run passed:

- three compatible workers before the run;
- concurrent exact-result rounds;
- active query cancellation with final state `CANCELED`;
- zero retained exchange/spill files after cancellation;
- worker loss during a running distributed scan;
- replacement worker and retry attempt with the exact `504,000,000` result;
- replacement catalog compatibility and three workers after recovery;
- bounded pressure with exact grouped results and no unexpected restart or
  retained-file growth; and
- exact post-recovery probe.

Credential-safe raw output is retained locally at
`tmp/track1-aks-fault-pressure-2026-10-14-rerun.json`; it records no token or
certificate material. A live skew-specific fault injection remains separate
from this loss and pressure qualification.
