# Track 1 repeated AKS fault and pressure qualification — 2026-10-15

This run repeats the current three-worker fault and pressure gate on the
release image. The credential-safe raw report is retained locally as
`tmp/aks-fault-pressure-repeat-2026-10-15.json`.

## Runtime

- Cluster: `kaveon-aks`, namespace `kaveon`
- Kubernetes: `1.35.7`
- Topology: three Engine workers
- Engine image: `kaveonacr.azurecr.io/kaveon-engine@sha256:c997febb6d14040bd9f26d21d1f96c90f33a6c5faf7a728cf2ba350e8181f06f`
- Failure mode: forced deletion of one worker during the distributed fault query
- Pressure: concurrency 4, three rounds, 12 exact requests

## Results

The harness passed all seven checks:

- three catalog-compatible workers before the run;
- concurrent exact results;
- active-query cancellation and cleanup;
- worker-loss retry, replacement, and exact recovery;
- bounded pressure without unexpected restarts or retained-file growth;
- no pre-existing exchange or spill files; and
- an exact post-recovery probe.

The run observed loss of `kaveon-worker-1`, replacement scheduling, and a retry
attempt with an exact result match. This is repeat evidence for the bounded
three-worker pod-loss gate and the pressure/cancellation behavior. It does not
qualify graceful node-drain retry, coordinator failover, or a sustained
multi-hour soak.
