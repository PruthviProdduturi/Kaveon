# Scale suite — KaveonDB on the 504 M-row table, 2026-09-15/16

> Twenty statements (`docs/qualification/scale-suite.json`), KaveonDB alone on the three worker nodes of `kaveon-test-aks`, one warm-up then three timed executions, medians in seconds. The Trino column is the 2026-09-15 time-to-answer pass on the same object (Trino 483, same nodes, alone). Every run's full JSON is beside this page. Engine digests: E1 `0b638f92…`, +E2 `a7224b91…`, +E4/E5/E7 `1b8c1bf0…`, full `a8a4603a…` (dev `5150045`).

The last column is the same statements on the dictionary-encoded rebuild of the file (`kaveon_events_enriched_v2/combined-v2.parquet`, 3.29 GB, Arrow schema stored, unique dictionaries). Trino's numbers were measured on the plain file; a Trino pass on the dictionary file is owed before any claim.

| Statement | E1 pruning, plain file | +E2 lanes | +E4/E5/E7 | full Engine, dictionary file | Trino | Target |
|---|---:|---:|---:|---:|---:|---:|
| `P04` | 89.01 | 66.20 | 71.27 | 12.11 | 33.84 | 20 |
| `F03` | 36.59 | 19.53 | 33.11 | 5.07 | 20.5 | 12 |
| `F04` | 21.70 | 13.11 | 23.21 | 4.28 | 10.32 | 6 |
| `F08` | 18.73 | 13.94 | 8.49 | 3.52 | 3.84 | 3 |
| `Y01` | 15.71 | 12.84 | 12.10 | 8.88 | 2.45 | 3 |
| `Y02` | 45.01 | 36.12 | 25.00 | 12.70 | 20.28 | 12 |
| `Y03` | 0.22 | 0.22 | 0.28 | 0.22 | 0.51 | 1 |
| `Y04` | 0.21 | 0.26 | 0.26 | 0.23 | 0.46 | 1 |
| `Y05` | 32.13 | 24.83 | 11.25 | 3.85 | 2.72 | 3 |
| `Y06` | 32.08 | 24.77 | 12.10 | 3.99 | 2.27 | 3 |
| `C02` | 35.95 | 31.48 | 20.02 | 11.30 | 3.24 | 3 |
| `C03` | 40.67 | 34.05 | 23.08 | 11.55 | 18.42 | 10 |
| `C04` | 29.31 | 24.48 | 45.11 | 8.26 | 13.35 | 8 |
| `count_star` | 0.81 | 0.87 | 0.73 | 0.86 | — | 1 |
| `sum_1col` | 8.03 | 6.25 | 1.56 | 1.45 | — | 3 |
| `sum_by_surface` | 30.92 | 25.02 | 11.10 | 4.19 | — | 5 |
| `sum_by_country` | 42.05 | 30.06 | 17.50 | 5.38 | — | 8 |
| `sum_by_pair` | 86.07 | 66.85 | 70.80 | 11.16 | — | 12 |
| `surface_chat_sum` | 5.26 | 4.43 | 3.10 | 2.24 | — | 3 |
| `one_day` | 0.73 | 0.65 | 0.62 | 0.47 | — | 1 |

**Now:** Kaveon is faster than Trino on **9 of 13** compared statements (2 of 13 at the E1 baseline yesterday), and 12 of 20 targets are met. Behind on the four shapes that read every row through a date predicate or group by day (`Y01`, `Y05`, `Y06`, `C02`): the predicate was compared per row on a dictionary column; `3e0934d` compares through the dictionary and gathers, pending its roll.

What changed between the columns, all on `dev`: E1 row-group pruning on inexact string statistics (`95eac30`); E2 parallel decoder lanes for ADLS objects (`c9de306`, `2528d63`); string keys interned and rows indexed by code (`45ed062`, `451be2e`); batch folds for SUM/AVG/MIN/MAX/COUNT and per-code folds for one or several string keys (`f1ccae8`, `be61987`); lane-side typed comparisons instead of a decoder row filter over object storage (`efc42bd`); dictionary-encoded columns end to end (`13c3479`, `5150045`); and the file itself rebuilt with unique dictionaries so Parquet keeps dictionary pages (`be61987`).
