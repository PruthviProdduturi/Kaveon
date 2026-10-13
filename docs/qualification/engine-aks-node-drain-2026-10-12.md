# AKS node-drain qualification — 2026-10-12

The `kaveon-aks` worker node `aks-workers-15909223-vmss000002` was cordoned and drained. Its `kaveon-worker-2` pod was evicted and recreated on the same worker pool. The replacement became Ready after the node was uncordoned; all three worker pods and the coordinator are Ready with zero container restarts. The node is schedulable again.

The machine-readable result is [engine-aks-node-drain-2026-10-12.json](engine-aks-node-drain-2026-10-12.json).

This qualifies node scheduling and StatefulSet replacement. It does not claim an in-flight query survived node loss; that remains covered separately by the worker-loss gate and is still distinct from node-drain query recovery.
