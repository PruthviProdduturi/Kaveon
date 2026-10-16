# AKS join spill-quota exhaustion qualification — 2026-10-15

The current three-worker Engine was run with a reversible worker spill quota of
64 KiB and a 4 MiB per-query memory budget. The bounded high-cardinality join
was submitted twice with one local lane, result caching disabled, and statistics
disabled. Both requests failed closed with HTTP 500 and the actionable error
`spill limit exceeded`; neither produced an opaque worker crash.

After the probe, the worker spill quota was restored to the production 4 GiB
value and the StatefulSet completed a clean rollout. All three workers were
Ready with zero restarts and zero retained `/tmp/spill` files. A normal
post-restore exact-repeat join returned the expected six-row result hash across
all three workers.

The raw report is retained in `track1-aks-join-spill-exhaustion-2026-10-15.json`.
This qualifies bounded spill-quota exhaustion and fail-closed cleanup. It does
not claim physical node-disk-full behavior, which remains a separate operational
gate.
