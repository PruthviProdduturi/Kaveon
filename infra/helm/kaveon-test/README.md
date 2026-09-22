# Private AKS engine test chart

The client-facing Service is `kaveon` (the Helm release name):

```powershell
kubectl -n kaveon port-forward service/kaveon 18443:8080
```

Open `https://localhost:18443/ui`. The coordinator Service remains for internal
discovery and its existing TLS DNS identity; this client alias requires no token,
certificate or workload changes. Direct TLS clients must use a certificate-covered
hostname rather than assuming the new Service name is in the certificate.

Deploy into namespace `kaveon` with release name `kaveon` to match the Bicep workload identity subject. Supply `image.repository`, immutable `image.digest` and `workloadIdentity.clientId`. To enable product transactions, first deploy `infra/bicep/environments/aks-product-transactions.bicep`, then supply its `storageAccountName`, `productTransactionContainer`, and `productCatalogPrefix` outputs. These are resource coordinates, not credentials. The workload identity has contributor access only on the dedicated product container and remains read-only on analytics containers.

Create existing Secret `kaveon-engine-auth` with keys `security.json`, `exchange-token` and `catalog-token`. The JSON uses the server's security schema, for example `{"principals":[{"principal":"test-admin","role":"admin","token":"<random token at least 32 bytes>"}]}`. Generate separate random credentials for every token domain. Do not put real credentials in values or version control.

Create existing TLS Secret `kaveon-engine-tls` with `tls.crt`, `tls.key` and `ca.crt`. `ca.crt` must contain the private issuing CA plus public roots needed for Azure HTTPS. For the default release/namespace, server certificate DNS SANs must cover `kaveon-coordinator.kaveon.svc.cluster.local` and `*.kaveon-workers.kaveon.svc.cluster.local`. Add the short coordinator service name if clients use it. The chart advertises stable worker pod DNS, not ephemeral IP addresses.

The engine's default reqwest transport uses native TLS/OpenSSL. The chart points `SSL_CERT_FILE` to the mounted CA bundle; certificate verification stays enabled. Validate worker heartbeats and a distributed query after deployment: HTTPS Kubernetes probes do not verify server certificates and therefore cannot prove internal TLS trust. Azure workload identity injects its projected token and Azure environment variables through the labeled pods and annotated ServiceAccount. An actual ADLS read must pass before declaring this integration ready.

Containers run as numeric UID/GID 10001 with a read-only root filesystem, no Linux capabilities, writable `/tmp`, and group-writable state volumes. The coordinator's SQLite catalog persists across pod replacements; running queries do not resume and this is not HA. Worker state, exchange spools and spills are ephemeral. Coordinator readiness uses `/ready`, which requires an initialized catalog; bootstrap through pod-local access before the coordinator Service has ready endpoints. Worker readiness uses `/health`, proving process health only: workers execute shipped fragments without a synchronized local catalog, so catalog-dependent `/ready` would prevent their headless DNS endpoints from being published. Probes do not demonstrate worker registration, ADLS authorization or query correctness; verify these separately with a distributed query. Drain queries before upgrades; the grace period alone does not provide draining.

## Memory and exchange settings

Every memory and disk budget the Engine reads from the environment is a chart value, per role, so `helm upgrade` reproduces the running StatefulSets. Byte values are strings.

| Value | Env var | Coordinator | Worker |
|---|---|---|---|
| `<role>.memory.queryLimitBytes` | `KAVEON_QUERY_MEMORY_LIMIT_BYTES` | 512 MiB | 3 GiB |
| `<role>.memory.admissionLimitBytes` | `KAVEON_MEMORY_ADMISSION_LIMIT_BYTES` | 2 GiB | 4 GiB |
| `<role>.exchange.spool` | `KAVEON_COORDINATOR_EXCHANGE_SPOOL` / `KAVEON_WORKER_EXCHANGE_SPOOL` | `false` | `true` |
| `<role>.exchange.diskLimitBytes` | `KAVEON_EXCHANGE_DISK_LIMIT_BYTES` | 24 GiB | 8 GiB |
| `<role>.exchange.queryDiskLimitBytes` | `KAVEON_EXCHANGE_QUERY_DISK_LIMIT_BYTES` | 16 GiB | 6 GiB |
| `workers.stateSizeLimit` | `/state` emptyDir `sizeLimit` | 32 GiB PVC | 16Gi |
| `spillDiskLimitBytes` | `KAVEON_HASH_SPILL_BYTES` | 4 GiB | 4 GiB |

`<role>` is `coordinator` or `workers`. The spool root is `/state/exchange` on both roles; workers also set `KAVEON_IPC_SPOOL_ROOT=/state` so received exchange payloads spool on the state volume rather than `/tmp` (the directory must exist, and `/state` is always mounted). Hash spill stays under `/tmp/spill` inside the 6 GiB `/tmp` emptyDir.

With `workers.exchange.spool: true`, each worker keeps the exchange partitions addressed to it on its own disk, so producers upload straight to the consuming worker and the coordinator carries no exchange traffic; `coordinator.exchange.spool` is therefore `false`. The coordinator limits remain sized for its 32 GiB state PVC so the coordinator-hosted spool can be re-enabled without retuning. `queryDiskLimitBytes` is one query's share of the node's spool.

The admission limit must fit inside the container memory limit less the Engine's headroom (the larger of 256 MiB and 15 %); the Engine refuses to start otherwise. The Engine reads the cgroup limit itself, so the chart never sets `KAVEON_PROCESS_MEMORY_LIMIT_BYTES`. When raising `workers.resources.limits.memory`, raise the worker budgets together with it.

## Resource groups and the demo posture

`resourceGroups.enabled` (default `false`) renders `resourceGroups.document` to a ConfigMap mounted read-only on the coordinator and named by `KAVEON_RESOURCE_GROUPS`, so the groups are Helm's rather than the coordinator's durable runtime copy (a `PUT /v1/admin/resource-groups` then fails on the read-only file; edit the values instead). The example document in `values.yaml` is the public demo's posture: `demo.enabled` and a `demo` group with `rate: {max_statements: 5, per_seconds: 21600, count: live}` — five live statements per principal per rolling six hours, counted from the audit ledger so a restart keeps the count, admins exempt, cache and statistics answers not counted. A self-hosted install leaves the block off. The portal chart's `api.demoMode` is the other half; see `docs/engine/governance.md`.

```powershell
helm upgrade --install kaveon infra/helm/kaveon-test --namespace kaveon --create-namespace --set image.repository=<registry>/kaveon-engine --set image.digest=sha256:<digest> --set workloadIdentity.clientId=<client-id> --set productTransactions.enabled=true --set productTransactions.account=<storage-account> --set productTransactions.container=<product-container> --set productTransactions.prefix=<product-prefix> --wait --timeout 10m
```

Use port-forwarding and a trusted CA for test access. No public load balancer or ingress is created. An ingress NetworkPolicy permits port 8080 from pods in the same namespace only; cross-namespace API access requires an explicit additional rule. Egress remains available for Azure identity, ADLS and DNS. Authenticated TLS is required on every engine endpoint. Confirm the cluster network plugin enforces NetworkPolicy.
