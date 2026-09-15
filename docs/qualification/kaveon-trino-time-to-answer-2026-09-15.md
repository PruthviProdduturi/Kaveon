# Time to answer on the 504 M-row table — KaveonDB versus Trino 483, 2026-09-15

> The product-level companion to the fixture benchmark. Same cluster, same three worker nodes, each engine alone, the thirteen corpus questions that the DLM answers with a live statement (the other sixty-seven serve from context in about 0.1 s and involve no engine). Same Parquet object for both engines: `OpenSource.kaveon_product.kaveon_events_enriched`, 504,000,000 rows, one 6.43 GB file, 168 row groups sorted by day. One warm-up, three timed executions, median. Trino reads through the read-only Hive catalog declared from the catalog manifest; Kaveon through the API bridge. Evidence beside this page: `kaveon-trino-time-to-answer-2026-09-15.json`.

## Result

| Question | Kaveon (s) | Trino (s) | Kaveon ÷ Trino | Result rows |
|---|---:|---:|---:|---:|
| telemetry actions by country and industry | 82.3 | 33.8 | 2.4× | 50 |
| telemetry queries run in Europe by country | 32.3 | 20.5 | 1.6× | 7 |
| telemetry actions in United States by surface | 20.8 | 10.3 | 2.0× | 6 |
| telemetry actions on the Chat surface by license | 23.0 | 3.8 | 6.0× | 4 |
| telemetry actions in July 2026 | 15.3 | 2.4 | 6.3× | 1 |
| telemetry sessions in 2026 by country | 44.2 | 20.3 | 2.2× | 26 |
| telemetry actions in 2025 | 10.5 | 0.5 | 20.8× | 1 |
| telemetry errors last 7 days | 6.9 | 0.5 | 14.9× | 1 |
| telemetry sessions by month | 29.8 | 2.7 | 10.9× | 28 |
| trend of telemetry actions over time | 29.5 | 2.3 | 13.0× | 28 |
| what about 2026? | 35.4 | 3.2 | 10.9× | 6 |
| and by platform | 40.4 | 18.4 | 2.2× | 3 |
| only for Mobile | 28.0 | 13.3 | 2.1× | 1 |

**Trino is faster on all thirteen; geometric mean 5.1×, range 1.6–21×.** This is the opposite of the 5 M-row fixture result from the same night (Kaveon 1.45× Trino on throughput) and it is the number that matters for a person waiting on a live question at scale.

## Where the time goes

The file's own statistics say most of it:

- **Row-group pruning.** `actions in 2025` and `errors last 7 days` match zero rows; every row group's `event_date` min/max says so. Trino answers in 0.5 s without reading data. Kaveon reads all 504 M rows: 10.5 s and 6.9 s. The same applies to `surface`, which is clustered within the file (`surface = 'Chat'`: Trino 3.8 s, Kaveon 23.0 s).
- **Decode throughput.** `actions in July 2026` touches every row but two columns; Trino 2.4 s, Kaveon 15.3 s. The afternoon profile put Kaveon's single-column scan floor at 6.5 s (~78 M rows/s) — the gap is in the Parquet reader and predicate evaluation, not in I/O.
- **String keys.** `GROUP BY country, industry`: Trino 33.8 s, Kaveon 82.3 s. Both pay for PLAIN-encoded string dimensions (this file was written with dictionaries off, before `e68294c`); Kaveon pays about 2.4× more per key.

## What this means for the product

- The DLM is the reason the product is fast: 67 of 80 corpus questions never reach an engine. With the cuboid cover (`9fd2966`, awaiting an AKS rebuild of the dataset) six of these thirteen — the pair and filter-plus-breakdown shapes — also move to context. The seven date-window shapes stay live and are exactly the ones row-group pruning would make sub-second.
- On live statements at this scale the Engine is behind Trino by 5×. That is an engine-reader problem with three named causes, not an architecture problem: pruning on row-group statistics, decode throughput, and grouping on dictionary indices. Each is measurable with the statements above.
- On the data side, the telemetry file should be rewritten with dictionary-encoded string columns now that the Engine groups on dictionary keys; both engines get cheaper, and the Engine gets the index path.

## Conditions and caveats

- Kaveon Engine `sha256:df9cfb7c…`, Trino 483 `sha256:db58cc93…`, three D4s_v3 workers each, one engine at a time, warm cache. Kaveon's column was measured after Trino's, in a Job on the same nodes.
- Trino's table is an external Hive table over the same object; no Delta log, no statistics collected (`ANALYZE` was not run), so Trino used only Parquet footer statistics — the same information the Engine has.
- Medians of three; the spread is in the JSON. Not a publication claim; a diagnosis.
