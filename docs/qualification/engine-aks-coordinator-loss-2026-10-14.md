# AKS coordinator-loss qualification — 2026-10-14

This qualification covers the failure contract for an in-flight statement when
the coordinator process is restarted. It does **not** claim active-query
survival or high availability: the expected result of the interrupted request
is a closed connection, followed by a clean post-restart probe.

| Item | Result |
| --- | --- |
| Source commit | `a4cc215f` |
| Engine image | `kaveonacr.azurecr.io/kaveon-engine@sha256:c997febb6d14040bd9f26d21d1f96c90f33a6c5faf7a728cf2ba350e8181f06f` |
| Cluster | `kaveon-aks` / `kaveon` / West US 2 |
| Coordinator and workers | Coordinator ready; three workers ready after restart |
| In-flight interruption | Observed; request failed closed when the coordinator was restarted |
| Post-restart exact probe | Passed (`SELECT COUNT(*)` returned `24`) |
| Exchange cleanup | Passed; retained coordinator exchange files after restart: `0` |
| Credentials or subscription policy changes | None |

The run was produced by
`scripts/qualify-aks-coordinator-loss.py` and is archived at
`tmp/engine-aks-coordinator-loss-a4cc215f.json` in the qualification workspace.
The Engine now supports an opt-in startup reclaim for a **dedicated** exchange
root (`KAVEON_EXCHANGE_RECLAIM_ON_START=true`). It removes only directories
named `kaveon-exchange-<UUID>` and leaves unrelated files in the root alone.
The AKS coordinator and workers are configured with the setting; the Helm
values and settings reference document the same contract.

This closes the coordinator-restart cleanup gate. It does not close the
remaining operational gaps: active-query survival, multi-coordinator
failover, node-drain qualification, sustained mixed-workload soak, or backup
and rollback rehearsal.
