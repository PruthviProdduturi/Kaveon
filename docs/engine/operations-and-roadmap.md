# Operations and roadmap

Engine exposes health/readiness, statement, query/cancellation, cluster/node, catalog, task/exchange, and `/ui` operational surfaces. Query history is process-local. Metrics remain optional where not measured; Kaveon never fabricates operator CPU, memory, blocked, network, or spill values.

Internal exchange uses a separate bearer token. The Engine supports authenticated
principals/roles, TLS, and optional Microsoft Entra authentication. The AKS test
deployment enables these controls. The CLI reuses Azure login, and the dashboard
shows the client and authenticated user while retaining the immutable principal
for ownership. Production qualification still requires reviewed tenant isolation,
credential/certificate rotation, network controls and operational recovery.

## Production gates

1. Review and requalify the combined build using the published readiness rubric.
2. Broaden pressure/skew and recovery coverage for admission, resource groups and spill.
3. Extend cloud/storage correctness coverage beyond the live ADLS fixture checks.
4. Qualify platform/Engine integration, migrations and identity behavior for production.
5. Fill missing per-operator and distributed scan telemetry; do not substitute result bytes for scan bytes.
6. Qualify sustained failure, retry, cancellation, concurrency, upgrade and backup/restore behavior.
7. Complete fair comparative benchmarks with identical data, SQL, resources, warmup and correctness gates.

See the [validation checkpoint](../engineering/checkpoint-2026-09-08.md),
[AKS deployment evidence](../engineering/aks-test-deployment.md),
[CLI guide](../guides/engine-cli.md) and
[remaining CLI compatibility gaps](../engineering/cli-compatibility.md).

Engine is a functioning distributed alpha, not yet a production-equivalent Trino replacement.
