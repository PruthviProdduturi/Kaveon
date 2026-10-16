# Track 1 AKS: graceful node-drain evidence

## Deployment

- Engine image: `kaveonacr.azurecr.io/kaveon-engine@sha256:6a4fa35e1d511bdd533d6d3ee22c331137775a0848544bfd0fd98c2e6dea5b18`
- Catalog snapshot: `sha256:652742525737d79eb579cdef4a8386b6fa6c9f3feedc60b197bd3406ab73d1f3`
- Topology: one coordinator and three workers.

## Observation

The qualification harness captured a baseline for a distributed 504M-row scan,
then cordoned and drained the node hosting the longest-running worker task while
the repeated query was running. The query returned the exact baseline result:
`COUNT(*) = 504000000`. The worker StatefulSet replacement received a new UID,
all three workers returned with the pinned catalog snapshot, and the node was
uncordoned in the cleanup path.

The run did **not** observe a retry attempt because Kubernetes evicted the pod
gracefully before the task failed. The harness therefore correctly marked the
strict worker-loss-retry gate as failed; this report is graceful-drain evidence,
not a retry qualification.

The complete report is retained in
`track1-aks-node-drain-graceful-2026-10-16.json`.

## Boundary

A forced worker-loss retry is qualified separately. Repeated node drains,
node loss during a long soak, and a drain that interrupts an in-flight task
remain open.
