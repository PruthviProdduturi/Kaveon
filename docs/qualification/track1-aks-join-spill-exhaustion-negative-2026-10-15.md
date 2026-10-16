# AKS join spill-exhaustion probe — negative evidence — 2026-10-15

A reversible worker-only quota probe set `KAVEON_HASH_SPILL_BYTES` to 64 KiB
for the bounded high-cardinality join. The query finished successfully without
writing spill files, so this probe did **not** exercise the join spill path and
is not a disk-exhaustion qualification.

The worker setting was restored to the production 4 GiB value. All three worker
pods returned Ready with zero restarts, and a post-restore exact-repeat join
probe returned the expected six-row hash on all three workers.

The raw negative probe report is retained in
`track1-aks-join-spill-exhaustion-negative-2026-10-15.json`. A live disk-
exhaustion test for a workload that is proven to spill remains open.
