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

## Gate still open

The full fault-pressure run was not marked passed. Its old NYC profile was
pointing at an `OpenSource.nyc_taxi` dataset absent from this fresh medallion
cluster; the verifier now has a medallion profile. A 10,000-row medallion join
completed before the forced-loss barrier, so it cannot prove retry under loss.
An attempt to replace the registered Parquet object with a 1M-row file also
correctly exposed a stale source-version/ETag precondition (HTTP 412); the
catalog must be re-registered at a new immutable object version before using
that larger workload. This is intentionally recorded as an open qualification
item rather than a product pass.

The local distributed evidence remains valid: exchange-loss exact retry at one
million rows, 37/37 semantic smoke cases, and the 60-second operational soak
are recorded in the adjacent qualification reports.
