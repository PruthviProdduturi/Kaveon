# Operations and roadmap

The Engine exposes health/readiness, statement (with per-request settings
and `SET SESSION` prefixes), query history and cancellation, cluster/node
(memory, admission and result-cache counters), catalog, cache, transaction,
task/exchange, and `/ui` operational surfaces; the [HTTP API
reference](../reference/api.md) lists them. Query history is process-local
(100 records). Metrics remain optional where not measured; Kaveon never
fabricates operator CPU, memory, blocked, network, or spill values.

Internal exchange uses a separate bearer token. The Engine authenticates
principals with roles, validates Entra bearer tokens, serves native TLS, and
applies per-principal, resource-group and memory admission limits; the AKS
test deployment enables all of them. The CLI reuses Azure login, and the
Engine UI shows the client and authenticated user while retaining the
immutable principal for ownership. Production qualification still requires
reviewed tenant isolation, credential and certificate rotation without
restart, network controls and operational recovery.

Every memory, disk and parallelism bound is a setting with a default from
the code and a value on the AKS cluster, listed in the [settings
reference](settings.md). The chart `infra/helm/kaveon-test` renders those
values per role, so `helm upgrade` reproduces the running StatefulSets.

## Production gates

1. Measure the hybrid final merge and the exchange changes on the cluster
   and complete the five-round ClickBench and TPC-H SF100 records under
   `docs/qualification/benchmark-program.md`.
2. Stream exchange decode on the consumer; qualify aggregate and join spill
   under skew and disk exhaustion.
3. Qualify a sustained mixed-workload soak, backup/restore of the catalog
   and an upgrade/rollback on the current image.
4. Extend storage coverage: plain Parquet directory tables (in progress),
   Delta reader protocol v2 features, S3 against a live bucket.
5. Close the remaining SQL refusals that matter to analysts, starting with
   residual semi joins (TPC-H Q21).
6. Qualify identity rotation, tenant isolation and the platform bridge for
   production.
7. Fill the remaining telemetry: live per-operator CPU and memory, spilled
   tables and groups on the final merge.

See the [validation checkpoint](../engineering/checkpoint-2026-09-08.md),
[AKS deployment record](../engineering/aks-test-deployment.md),
[readiness rubric](../engineering/engine-readiness-qualification.md),
[CLI guide](../guides/engine-cli.md) and
[remaining CLI compatibility gaps](../engineering/cli-compatibility.md).

The Engine is a functioning distributed alpha, measured against Trino on a
matched three-worker cluster (`docs/qualification/`), not yet a
production-qualified engine.
