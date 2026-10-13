# AKS QUALIFY deployment qualification — 2026-10-13

The Engine StatefulSets were rolled to ACR digest `sha256:fe468d403a53b82667427952db8121cd491b30238455c83a393b7e6cdfe63570`, built from the current repository HEAD. On that exact image, a partitioned `ROW_NUMBER()` query with `QUALIFY rn = 1` finished successfully and matched the equivalent derived-table `WHERE rn = 1` query exactly.

Machine-readable evidence: [engine-aks-qualify-latest-2026-10-13.json](engine-aks-qualify-latest-2026-10-13.json)
