
## 2026-10-08 — larger memory control (rejected)

A paired control with 1 GiB per-query memory and 8 GiB cluster admission completed exactly but did not improve the workload: q33 `20.239 s` and q35 `14.152 s`. The standard 512 MiB / 2 GiB configuration remains the qualified default; the bottleneck is representation and spill work, not simply the memory cap.

## 2026-10-08 — final spill-run compaction (rejected)

A qualification-only final-merge compaction pass reduced the number of spill runs before replay. It preserved exact q33/q35 results, but the paired timings were q33 `19.420 s` and q35 `13.842 s`; the q35 regression outweighed the q33 improvement. The pass was reverted and remains disabled.

## 2026-10-08 — exchange chunk-buffer split (rejected)

Replacing front-drain chunking with ownership-preserving `Vec::split_off` passed all 22 exchange tests and preserved both controls, but did not improve the pair: q33 `19.738 s`, q35 `13.696 s`. The change was reverted; Arrow state encoding/decoding and final aggregation remain the measured bottleneck.

## 2026-10-08 — exchange fan-out eight (rejected)

Increasing exchange partitions from four to eight reduced per-task input but did not improve the pair: q33 `20.031 s`, q35 `13.823 s`, with exact results. The four-partition default remains the qualified setting.

## 2026-10-08 — type-gated raw numeric exchange (rejected)

The raw-row exchange was narrowed to direct scans whose group keys are numeric, so q33 could use it while q35's UTF-8 URL grouping stayed on grouped states. Planner tests passed and both controls remained exact, but the pair was still not faster: q33 `19.528 s`, q35 `13.680 s`. The experimental modes were reverted; grouped-state remains the only default wire contract.
