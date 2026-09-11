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

Containers run as numeric UID/GID 10001 with a read-only root filesystem, no Linux capabilities, writable `/tmp`, and group-writable state volumes. Coordinator SQLite and exchange files persist across pod replacements; running queries do not resume and this is not HA. Worker state and spills are ephemeral. Coordinator readiness uses `/ready`, which requires an initialized catalog; bootstrap through pod-local access before the coordinator Service has ready endpoints. Worker readiness uses `/health`, proving process health only: workers execute shipped fragments without a synchronized local catalog, so catalog-dependent `/ready` would prevent their headless DNS endpoints from being published. Probes do not demonstrate worker registration, ADLS authorization or query correctness; verify these separately with a distributed query. Drain queries before upgrades; the grace period alone does not provide draining.

```powershell
helm upgrade --install kaveon infra/helm/kaveon-test --namespace kaveon --create-namespace --set image.repository=<registry>/kaveon-engine --set image.digest=sha256:<digest> --set workloadIdentity.clientId=<client-id> --set productTransactions.enabled=true --set productTransactions.account=<storage-account> --set productTransactions.container=<product-container> --set productTransactions.prefix=<product-prefix> --wait --timeout 10m
```

Use port-forwarding and a trusted CA for test access. No public load balancer or ingress is created. An ingress NetworkPolicy permits port 8080 from pods in the same namespace only; cross-namespace API access requires an explicit additional rule. Egress remains available for Azure identity, ADLS and DNS. Authenticated TLS is required on every engine endpoint. Confirm the cluster network plugin enforces NetworkPolicy.
