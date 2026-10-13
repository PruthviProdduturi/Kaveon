# AKS native ANALYZE restart qualification — 2026-10-12

The current `kaveon-aks` coordinator passed a two-phase native `ANALYZE` durability check on the deployed Engine image. Before the restart, `OpenSource.ai_benchmarks.leaderboard` reported an exact row count of 34 and a current durable statistic. The coordinator StatefulSet was then restarted and became Ready in 13 seconds. The post-restart check again reported the exact 34-row statistic and current metadata.

Reports:

- [Before restart](engine-aks-native-analyze-2026-10-12-before.json)
- [After restart](engine-aks-native-analyze-2026-10-12-after.json)

This closes the native-statistics persistence check for this table and image. It does not close exchange-spool cleanup, node-drain recovery, sustained soak, cloud-storage fault injection, or multi-coordinator consistency.
