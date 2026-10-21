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

