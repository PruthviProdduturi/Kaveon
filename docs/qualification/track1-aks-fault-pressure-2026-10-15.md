# Track 1 AKS fault and pressure qualification — 2026-10-15

This run used the personal West US 2 AKS cluster with the three-worker Engine
deployment. The credential-safe raw report is retained locally as
`tmp/aks-fault-pressure-2026-10-15.json`; it contains no credential material.

## Runtime

- Cluster: `kaveon-aks`, namespace `kaveon`
- Kubernetes: `1.35.7`
- Workers: three Ready StatefulSet workers before and after the run
- Engine image: `kaveonacr.azurecr.io/kaveon-engine@sha256:c997febb6d14040bd9f26d21d1f96c90f33a6c5faf7a728cf2ba350e8181f06f`
- Failure mode: forced deletion of one worker pod while its task was running
- Pressure: concurrency 4, two rounds

## Results

The harness passed every check:

- concurrent exact results;
- active-query cancellation returned `CANCELED` and left zero exchange/spill
  files;
- worker loss was observed during the distributed scan;
- replacement scheduling and retry completed with the exact result
  `COUNT(*) = 504,000,000`;
- the replacement worker returned catalog-compatible;
- bounded grouped pressure completed without unexpected restarts, admission
  refusals, or retained-file growth; and
- the exact post-recovery probe passed.

The forced-loss query ran in 43.93 seconds after the worker was terminated and
returned the same result hash as its 14.68-second baseline. The grouped pressure
queries exercised three stages and all three workers. This qualifies the
declared three-worker loss, retry, cancellation, and bounded-pressure gate for
this image. It does not qualify coordinator failover, a sustained multi-hour
soak, or a general skew-splitting optimizer.

## Node-drain follow-up

A complementary `--failure-mode node-drain` run was attempted against the same
cluster and image. The worker was replaced and the 504M-row result remained
exact, but the harness did not observe a retry attempt: graceful eviction let
the task finish without exercising lease requeue. The run therefore failed the
`worker_loss_exact_retry_and_recovery` gate and is retained as negative evidence
in `tmp/aks-fault-pressure-node-drain-2026-10-15.json`. Track 1 must not treat
node-drain recovery as qualified until a controlled eviction produces an
observed retry.
