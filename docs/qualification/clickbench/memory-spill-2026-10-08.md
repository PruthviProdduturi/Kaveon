# ClickBench memory and spill audit — 2026-10-08

This note records the memory-pressure evidence behind the final-merge spill
change in `c7f8a803`. It is diagnostic evidence, not a ClickBench headline
comparison: the runs below use the local Docker stack and different lane
profiles from the historical Trino controls.

## Findings

The query memory pool is enforcing its ceiling. On the historical q35 runs,
the 512 MiB profile reached a peak of approximately 512 MiB and wrote 4.32 GiB
over 1,600 spill runs. The 2 GiB profile peaked at approximately 2.15 GiB and
wrote 3.07 GiB over 208 runs. The large `memory_reservation_bytes` values in
the records are cumulative reservation attempts, not simultaneously live
memory; they do not demonstrate over-admission.

The current fresh profile confirms the other failure boundary: q35 with a 2 GiB
ceiling fails in partial aggregation when roughly 2.05 GiB is reserved. The
final merge is not reached, so final-merge compaction cannot improve that run.
Fresh q33 with the same 2 GiB / four-lane local profile completed in 41.9s with
the expected result rows. These times are profile-specific and are not a
Trino or ClickHouse comparison.

## Change and safety gate

Final-merge spill runs now use the existing bounded size-tiered compactor. A
memory refusal during compaction restores both source runs and allows the
final reader to continue with the un-compacted files. Compaction counters are
incremented only after the replacement run is committed. The regression test
`spill_compaction_refusal_keeps_the_original_runs` verifies that the source
files and byte accounting survive a refusal.

The next performance gate is to reduce partial-aggregate memory or select a
lower-memory partial strategy before claiming a q35 improvement. Raising the
query limit alone is not an optimization and is excluded from this audit.
