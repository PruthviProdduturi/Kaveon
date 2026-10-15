# Track 1 AKS exchange and ADLS soak — 2026-10-15

This run exercises the current Engine image against the AKS coordinator and
ADLS-backed catalog with a mixed exact-result workload. The raw report is
retained locally as `tmp/aks-current-soak-2026-10-15.json`; it contains no
credential material.

## Runtime

- Cluster: `kaveon-aks`, namespace `kaveon`
- Kubernetes: `1.35.7`
- Topology: three Engine workers
- Engine image: `kaveonacr.azurecr.io/kaveon-engine@sha256:c997febb6d14040bd9f26d21d1f96c90f33a6c5faf7a728cf2ba350e8181f06f`
- Duration: 300 seconds
- Concurrency: 4

## Results

The run completed 416 requests with 416 exact results and zero failures. All
workers remained Ready, no unexpected restarts occurred, and coordinator
exchange plus worker spill retention remained zero before and after the run.

The workload covered exact counts and grouped results across the large public
dataset, NYC borough data, and AI benchmark data. This qualifies bounded
sustained mixed workload behavior for the current three-worker image and
ADLS-backed reads.

## Boundary

This does not close the remaining Track 1 exchange gate by itself. Explicit
cloud-storage fault injection, quota rejection, orphan-spool recovery, and a
longer endurance run remain separate evidence requirements.
