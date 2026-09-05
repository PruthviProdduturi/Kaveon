# Engine architecture and startup

## Runtime

The coordinator accepts SQL, resolves catalog objects, builds logical and physical plans, creates a validated stage DAG, serializes versioned executable fragments, assigns tasks, and collects the root result. Workers advertise routable addresses, execute fragments over Arrow `RecordBatch` streams, exchange Arrow IPC partitions, and report measured task results.

```text
CLI / future Studio and DLM
             |
Coordinator: catalog -> SQL -> optimizer -> stages -> scheduler
             |                                      |
             v                                      v
Workers: scan -> vector operators -> Arrow exchange -> result
             |
Local Parquet and Delta plus ADLS Gen2 Parquet today; cloud Delta, S3, and Iceberg pending
```

## Crates

| Crate | Responsibility |
|---|---|
| `core` | Errors, expressions, plans, operators, fragments, exchange, telemetry, memory, and catalog contracts |
| `storage` | Streaming Parquet/local Delta reads, pruning, metrics, and deterministic splits |
| `exec` | Vectorized operators, partitioning, memory-aware paths, and spill infrastructure |
| `sql` / `optim` | Parsing, logical planning, safe rewrites, and predicate conversion |
| `catalog` | Durable SQLite/WAL definitions, schemas, revisions, lifecycle, and audit |
| `server` | Coordinator/worker APIs, scheduler, fragments, exchange, lifecycle, and Engine UI |
| `cli` | Remote-first interactive/one-shot client; explicit embedded local mode |
| `python` | PyO3 scaffold; not integrated with the platform API |

## Startup and boundaries

Server startup loads configuration, migrates the native catalog, reconstructs the planning snapshot, initializes cluster/lifecycle/exchange state, and binds HTTP. Readiness requires usable catalog state; liveness only proves the process responds. Docker Compose supplies one coordinator, multiple workers, a shared exchange token, and persistent coordinator metadata.

Studio and FastAPI do not yet route analytical queries through Engine. Root results are synchronously materialized, metadata is single-coordinator, and end-user Engine authentication/TLS is not production-qualified.
