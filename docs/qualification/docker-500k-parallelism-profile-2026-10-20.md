# Docker 500k parallelism profile — 2026-10-20

The current-source image was run with the two-worker distributed differential
at local parallelism 1 and 4. Both runs passed all 12 exact-result cases.

| Query shape | Parallelism 1 | Parallelism 4 |
|---|---:|---:|
| High-cardinality groups | 1409 ms | 1410 ms |
| Grouped join | 384 ms | 347 ms |
| Join | 269 ms | 251 ms |
| Grouped sum | 189 ms | 215 ms |

The high-cardinality aggregate did not improve with four local threads, so its
current gap is not explained by the worker thread setting. Future optimization
should profile hash aggregation, exchange serialization, and final merge work
directly. These timings are diagnostic only and do not establish a general
performance ranking.


## 2026-10-20 rerun with opt-in partial controls

A fresh two-worker, 500k-row extended run on `kaveon-engine:qualification-current`
remained exact on all 12 cases. The default control measured high-cardinality
aggregation at **1575 ms**. Enabling both existing opt-in controls,
`KAVEON_COALESCE_PARTIAL_ROWS=524288` and `KAVEON_PARTIAL_ROUND_ROBIN=1`,
measured **1479 ms** for the same case. That is a single diagnostic sample,
not a stable improvement; the controls remain opt-in and no default was changed.
The result confirms that local partial dispatch and batch coalescing alone do
not close the exchange/final-merge gap. A direct grouped-state exchange remains
the next structural target.
