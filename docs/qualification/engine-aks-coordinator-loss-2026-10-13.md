# AKS coordinator-loss qualification — 2026-10-13

The current Engine image passed a fail-closed coordinator-loss qualification. A 504M-row distributed scan was observed in `RUNNING`, then the coordinator StatefulSet was restarted. The in-flight request was interrupted with a connection termination rather than returning a partial result. After the coordinator became Ready, a fresh loopback connection returned the exact dashboard count of 24; all three workers were Ready and retained coordinator exchange files did not grow.

Machine-readable evidence: [engine-aks-coordinator-loss-2026-10-13.json](engine-aks-coordinator-loss-2026-10-13.json)

This qualifies clean interruption, cleanup, and post-restart service recovery. It does not claim that an in-flight query survives coordinator loss; the current contract is fail-closed interruption with a fresh-query recovery path.
