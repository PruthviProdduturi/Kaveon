# Engine architecture and startup

## Runtime

The coordinator accepts SQL, resolves catalog objects, binds the statement
against the catalog, builds logical and physical plans, creates a validated
stage DAG, serializes versioned executable fragments, assigns tasks, and
collects the root result. Workers advertise routable addresses, execute
fragments over Arrow `RecordBatch` streams on several threads, stream Arrow
IPC exchange partitions to the consuming node while the task runs, and
report measured task results.

```text
CLI / Studio and DLM through the API bridge
             |
Coordinator: catalog -> SQL -> binder -> optimizer -> stages -> scheduler
             |  admission queue, result cache, query history
             v
Workers: scan lanes -> vector operators -> streamed Arrow exchange -> result
             |
Parquet, Delta (v1 checkpoints) and Iceberg on local disk and ADLS Gen2;
S3 through the same reader, not qualified
```

## Crates

| Crate | Responsibility |
|---|---|
| `core` | Errors, expressions, plans, operators, fragments, exchange, telemetry, memory (query pools, admission queue, process guard), and catalog contracts |
| `storage` | Streaming Parquet, Delta and Iceberg readers on local disk and object stores, pruning, decoder lanes, metrics, deterministic splits, the ADLS conditional commit for the product store |
| `exec` | Vectorized operators: columnar hash aggregate, flushing partials, hybrid final merge, DISTINCT, joins, semi/anti joins, set operations, window, Sort/TopN, partitioned spill, local parallelism |
| `sql` / `optim` | Parsing and logical planning; the binder (`kaveon_optim::binder`), filter pushdown, projection and join pruning, exact-statistics broadcast choice |
| `catalog` | Durable SQLite/WAL definitions, schemas, revisions, lifecycle, audit, and the product manifest |
| `server` | Coordinator/worker APIs, security, scheduler, fragments, exchange stores and spools, result store and cache, per-request settings, lifecycle, the Engine UI, the TPC-H coverage gate and the differential sweep as tests |
| `cli` | Remote-first interactive/one-shot client; explicit embedded local mode (`--local`), which plans without the binder |
| `python` | PyO3 scaffold (19 lines); not used by the platform, which reaches the Engine over HTTPS |

## Startup and boundaries

Server startup loads configuration (`/etc/kaveon/config.toml` or the first
argument; environment overrides, see the [settings reference](settings.md)),
reads the container's cgroup memory limit, migrates the native catalog,
reconstructs the planning snapshot, initializes cluster/lifecycle/exchange
state and the result cache, and binds HTTP or HTTPS. Readiness on the
coordinator requires usable catalog state; worker readiness proves process
health only. Docker Compose supplies one coordinator, two workers, a shared
exchange token, and persistent coordinator metadata; the AKS chart
(`infra/helm/kaveon-test`) supplies one coordinator and three workers with
TLS, `KAVEON_SECURITY_JSON`, workload identity for ADLS and per-role memory
and exchange budgets.

Studio and FastAPI reach the Engine through the API's opt-in HTTPS bridge
(`KAVEON_ENGINE_URL`, `api/services/engine_bridge.py`), which delegates the
authenticated user's identity and role with the bridge token: SQL Lab
statements on Engine catalogs, DLM context builds, catalog registration and
the Engine console in Studio run on it where it is configured. Root results
are materialized synchronously unless the client requests paged delivery,
metadata is single-coordinator, and the Engine's authentication and TLS are
implemented but not production-qualified (rotation without restart, tenant
isolation and row/column policies remain gates).
