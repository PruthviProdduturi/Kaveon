# Query troubleshooting

Kaveon diagnoses slow statements from measured evidence. A single elapsed time
is not enough to justify a SQL rewrite, a layout change, or more capacity.

## 1. Identify the exact run

Use the query ID shown by Studio, the CLI, or the HTTP response:

```text
GET /v1/query/{query_id}
GET /v1/query/{query_id}/comparison
```

The record is owner-scoped. The comparison endpoint uses successful,
non-cache executions with the same normalized SQL, catalog, and schema. It
returns minimum, median, P95, maximum, current duration, cache-hit exclusions,
and recent admission-pressure events.

## 2. Separate the time

The query detail page shows measured analysis, planning, execution, and result
serialization phases. `admission_wait_ms` is the time spent waiting for memory
admission before execution and is not part of `elapsed_ms`.

- A large admission wait points to queue pressure or a resource-group limit.
- A large execution phase points to scans, exchange, operators, or spills.
- A large serialization phase points to result size or client delivery.
- Missing timings mean that the path did not measure that phase; they are not
  estimated values.

## 3. Check cache and storage locality

`execution.mode = cache` means the result came from the coordinator result
cache. Do not compare that duration with a query that performed the work.
Storage telemetry reports:

- `remote_bytes_read`: bytes fetched from ADLS/object storage;
- `local_bytes_read`: bytes fetched from local Parquet files;
- `compressed_bytes_selected` and `compressed_bytes_read`: selected data and
  bytes actually requested by the decoder;
- row-group, partition, Bloom-filter, and decoder-cache counters.

Absent values are unmeasured. Kaveon does not turn an unavailable counter into
zero.

## 4. Find the hotspot

The Stages tab exposes task-level elapsed time, CPU, queue time, peak memory,
spill bytes, exchange activity, output rows, and Arrow bytes. Compare the
slowest tasks with the stage average:

- queue time or repeated admission waits indicate resource pressure;
- exchange decode/hash/copy time indicates data movement overhead;
- spill bytes indicate a bounded-memory path, not automatically a defect;
- a wide task spread indicates skew or an uneven split assignment;
- high scan bytes with little pruning suggests projection, filtering, or
  physical layout work.

## 5. Choose one evidence-based action

- **Leave it alone** when the run is near the historical median and meets its
  target.
- **Investigate pressure** when comparable runs show admission waits or the
  current run is above P95 during concurrent load.
- **Rewrite SQL** only when the plan or counters show avoidable scans, joins,
  sorts, or repeated computation.
- **Improve layout/statistics** when pruning and row-group evidence show that
  the required query reads substantially more data than necessary.
- **Change capacity** only after recurring pressure is demonstrated at the
  workload concurrency that must be supported.

Change one thing at a time. Re-run with equivalent inputs and cache conditions,
then verify rows, NULLs, duplicates, and ordering as well as elapsed time.

## What Kaveon does not claim

The troubleshooting surface reports only counters emitted by the coordinator,
workers, and storage readers. It does not infer operator CPU, network, spill,
or storage values from elapsed time. In-flight operator attribution and longer
retention than the configured query journal remain future work.
