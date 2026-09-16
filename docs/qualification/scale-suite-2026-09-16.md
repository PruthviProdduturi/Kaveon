# Scale suite — KaveonDB on the 504 M-row table, 2026-09-15/16

> Twenty statements (`docs/qualification/scale-suite.json`), KaveonDB alone on the three worker nodes of `kaveon-test-aks`, one warm-up then three timed executions, medians in seconds. The Trino column is the 2026-09-15 time-to-answer pass on the same object (Trino 483, same nodes, alone, plain file). Every run's full JSON is beside this page.

Columns left to right are successive Engine digests on `dev`: E1 pruning `0b638f92…`; +E2 lanes `a7224b91…`; +E4/E5/E7 `1b8c1bf0…`; then the dictionary-encoded rebuild of the file (`kaveon_events_enriched_v2/combined-v2.parquet`, 3.29 GB, Arrow schema stored) on `a8a4603a…`; +dictionary comparisons `08c8edbb…`; +scalar literal comparisons `19bca2fe…` (dev `1c00593`).

| Statement | E1, plain | +E2 | +E4/5/7 | dictionary file | +dict cmp | +scalar literals | Trino | Target |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| `P04` | 89.01 | 66.20 | 71.27 | 12.11 | 11.50 | 11.17 | 33.84 | 20 |
| `F03` | 36.59 | 19.53 | 33.11 | 5.07 | 5.47 | 4.62 | 20.5 | 12 |
| `F04` | 21.70 | 13.11 | 23.21 | 4.28 | 4.17 | 4.01 | 10.32 | 6 |
| `F08` | 18.73 | 13.94 | 8.49 | 3.52 | 3.35 | 2.60 | 3.84 | 3 |
| `Y01` | 15.71 | 12.84 | 12.10 | 8.88 | 9.15 | 2.87 | 2.45 | 3 |
| `Y02` | 45.01 | 36.12 | 25.00 | 12.70 | 12.32 | 6.48 | 20.28 | 12 |
| `Y03` | 0.22 | 0.22 | 0.28 | 0.22 | 0.25 | 0.21 | 0.51 | 1 |
| `Y04` | 0.21 | 0.26 | 0.26 | 0.23 | 0.28 | 0.25 | 0.46 | 1 |
| `Y05` | 32.13 | 24.83 | 11.25 | 3.85 | 3.82 | 3.66 | 2.72 | 3 |
| `Y06` | 32.08 | 24.77 | 12.10 | 3.99 | 3.95 | 5.06 | 2.27 | 3 |
| `C02` | 35.95 | 31.48 | 20.02 | 11.30 | 11.38 | 5.84 | 3.24 | 3 |
| `C03` | 40.67 | 34.05 | 23.08 | 11.55 | 11.59 | 5.60 | 18.42 | 10 |
| `C04` | 29.31 | 24.48 | 45.11 | 8.26 | 6.60 | 4.58 | 13.35 | 8 |
| `count_star` | 0.81 | 0.87 | 0.73 | 0.86 | 0.80 | 0.78 | — | 1 |
| `sum_1col` | 8.03 | 6.25 | 1.56 | 1.45 | 1.40 | 1.29 | — | 3 |
| `sum_by_surface` | 30.92 | 25.02 | 11.10 | 4.19 | 3.93 | 3.83 | — | 5 |
| `sum_by_country` | 42.05 | 30.06 | 17.50 | 5.38 | 5.44 | 5.15 | — | 8 |
| `sum_by_pair` | 86.07 | 66.85 | 70.80 | 11.16 | 11.18 | 11.01 | — | 12 |
| `surface_chat_sum` | 5.26 | 4.43 | 3.10 | 2.24 | 2.28 | 1.25 | — | 3 |
| `one_day` | 0.73 | 0.65 | 0.62 | 0.47 | 0.47 | 0.40 | — | 1 |

**Now:** Kaveon is faster than Trino on **9 of 13** compared statements (2 of 13 at the E1 baseline) and meets **17 of 20** targets. The four where Trino still leads are within 0.5–2.6 s: `Y01` 2.87 vs 2.45, `Y05` 3.66 vs 2.72, `Y06` 5.06 vs 2.27, `C02` 5.84 vs 3.24 — the all-rows date window and by-day shapes, where the remaining cost is the per-day group fold and one column decode; the same shapes were 16–36 s two days ago.

Trino's column was measured on the plain file; a Trino pass on the dictionary file is owed before any public claim, and exact `COUNT(DISTINCT user_id)` per surface still exceeds a worker's memory reservation.

What changed between the columns, all on `dev`: E1 row-group pruning on inexact string statistics (`95eac30`); E2 parallel decoder lanes for ADLS objects (`c9de306`, `2528d63`); string keys interned and rows indexed by code (`45ed062`, `451be2e`); batch folds for SUM/AVG/MIN/MAX/COUNT and per-code folds for one or several string keys (`f1ccae8`, `be61987`); lane-side typed comparisons instead of a decoder row filter over object storage (`efc42bd`); dictionary-encoded columns end to end (`13c3479`, `5150045`, `3e0934d`); the executor comparing a column with a literal as a scalar, through the dictionary (`1c00593`); and the file rebuilt with unique dictionaries so Parquet keeps dictionary pages (`be61987`).
