
## 2026-10-08 — larger memory control (rejected)

A paired control with 1 GiB per-query memory and 8 GiB cluster admission completed exactly but did not improve the workload: q33 `20.239 s` and q35 `14.152 s`. The standard 512 MiB / 2 GiB configuration remains the qualified default; the bottleneck is representation and spill work, not simply the memory cap.
