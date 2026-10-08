
## 2026-10-08 — larger memory control (rejected)

A paired control with 1 GiB per-query memory and 8 GiB cluster admission completed exactly but did not improve the workload: q33 `20.239 s` and q35 `14.152 s`. The standard 512 MiB / 2 GiB configuration remains the qualified default; the bottleneck is representation and spill work, not simply the memory cap.

## 2026-10-08 — final spill-run compaction (rejected)

A qualification-only final-merge compaction pass reduced the number of spill runs before replay. It preserved exact q33/q35 results, but the paired timings were q33 `19.420 s` and q35 `13.842 s`; the q35 regression outweighed the q33 improvement. The pass was reverted and remains disabled.
