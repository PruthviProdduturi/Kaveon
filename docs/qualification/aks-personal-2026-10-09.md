# Personal AKS distributed qualification — 2026-10-09

This is a short-lived qualification deployment in the personal Azure
subscription `4ed07f02-b111-4eea-98ce-1c177d573a51`, resource group `kaveon-rg`.
The Engine image was pinned to digest
`sha256:d3bec07a0f30476be6967e544b4c653f1b6d274ba44ae9fdc0c587b7d23f18e3`.
No subscription-level policy was changed.

## Verified

- AKS `kaveon-test-aks` reached `Succeeded`/`Running` on Kubernetes 1.35.7.
- One coordinator and three worker pods were Ready on separate worker nodes.
- The private query bundle passed `scripts/verify-aks-test-results.py`:
  six exact medallion SQL results, three active workers, and unauthenticated
  catalog access rejected.
- The coordinator and workers reported a common catalog snapshot after the
  controlled worker restart and synchronization wait.
- The Helm chart uses the shared `kaveon-engine-auth` Secret; the fault-pressure
  verifier now accepts both the current shared Secret and the older coordinator
  Secret name without recording credentials.

## Fault and pressure gate

The fixture-aware verifier passed with the coordinator-backed exchange spool:

- three catalog-compatible workers before execution;
- 12 concurrent exact-result requests (four-way concurrency, three rounds);
- forced StatefulSet worker loss during a distributed self-join;
- exact retry and replacement recovery, with three compatible workers restored;
- 12 bounded pressure requests with no unexpected restart or retained-file
  growth; and
- an exact post-recovery probe.

The run is recorded at
`tmp/aks-fault-pressure-personal/medallion-central-spool.json` (private,
untracked). The local distributed evidence remains valid: exchange-loss exact
retry at one million rows, 37/37 semantic smoke cases, and the 60-second
operational soak are recorded in the adjacent qualification reports.

The 1M-row object replacement attempt correctly returned an HTTP 412 stale
source-version/ETag error. That is the expected protection for immutable
registered objects; a larger benchmark must be registered at a new object
version rather than overwriting a path in place.
