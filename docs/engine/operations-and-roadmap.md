# Operations and roadmap

Engine exposes health/readiness, statement, query/cancellation, cluster/node, catalog, task/exchange, and `/ui` operational surfaces. Query history is process-local. Metrics remain optional where not measured; Kaveon never fabricates operator CPU, memory, blocked, network, or spill values.

Internal exchange uses a shared bearer token. Catalog mutation uses a separate admin token and actor header. Production still requires end-user Entra authentication/authorization, TLS, network policy, tenant isolation, credential rotation, and audited identity propagation.

## Production gates

1. Commit and reverify the active SQL milestone.
2. Add queued admission and resource-group policy above the current hard process/query limits.
3. Add partitioned aggregate and join spill.
4. Validate ADLS Gen2 Parquet against real storage, then add cloud Delta-log replay.
5. Bridge platform source registration to Engine activation.
6. Route SQL Lab and deterministic DLM execution through Engine.
7. Add live per-operator CPU, memory, spill, exchange, and stage metrics.
8. Qualify failure, retry, cancellation, skew, concurrency, and upgrades on at least five AKS workers.
9. Benchmark against Trino/Fabric SQL Endpoint with identical data, SQL, resources, warmup, and correctness gates.

Engine is a functioning distributed alpha, not yet a production-equivalent Trino replacement.
