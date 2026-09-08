# AKS deployment plan

Status: planning only, 2026-09-08. No Azure resources, contexts, or deployments were changed for this plan. Local qualification is not AKS production qualification.

## Decision and inventory

Use Bicep for reproducible Azure infrastructure and Helm for Kubernetes workloads. Bicep is not an AKS requirement, but extending the existing `infra/bicep` investment is preferable to introducing a second infrastructure stack. Do not execute the existing production template as an AKS deployment.

| Existing component | AKS implication / missing work |
| --- | --- |
| `infra/bicep/environments/production.bicep` and modules for ACR, PostgreSQL, Key Vault, Log Analytics and Container Apps | Reuse or reference verified resources; add AKS, network, identities and role assignments. Current template has fixed names, ACR admin-password handling and a placeholder Key Vault principal; it needs reconciliation with live state before reuse. |
| Engine, API and Studio Dockerfiles; root Compose; container/Engine CI | Images and local integration foundation exist. Add immutable image publication, scanning, AKS chart rendering and staging validation. Compose development defaults must not become production values. |
| Engine coordinator and worker TOML files | Translate into chart configuration, discovery, stable worker identity, health probes and explicit resource/disk limits. No Helm chart or Kubernetes manifests currently exist. |
| `.github/workflows/deploy.yml` | A push to `dev` currently builds API in ACR and updates **Container Apps**. It does not deploy AKS. Keep the eventual AKS release explicit and environment-gated; account for this existing push side effect. |
| Native SQLite/WAL catalog and coordinator exchange spool | One coordinator only. Persist catalog and spool; do not imply multi-coordinator HA or recovery of active queries after coordinator restart. |
| API metadata PostgreSQL, Engine bridge and credential keyring | Preserve metadata migrations, actor attribution and separate authorities. Back up database and encryption keys together. |

Some older Engine documentation predates the current API bridge/auth/TLS implementation. Validate deployment values against `engine/crates/server/src/config.rs`, `api/services/engine_bridge.py`, and `api/services/credentials.py`.

## Proposed implementation boundaries

Extend `infra/bicep/modules/` with network/private DNS, AKS/node pools, workload identities/federation, scoped role assignments and optional storage/private endpoints. Add a parameterized AKS environment entry point, referencing existing ACR/PostgreSQL/Key Vault where selected. Do not redeclare existing resources without a reviewed ownership/migration plan. Pin a supported AKS version and VM sizes only after checking target-region availability and quota.

Create `infra/helm/kaveon/` for coordinator StatefulSet (one replica), worker StatefulSet/headless discovery, private Services, ConfigMaps, secret references, ServiceAccounts, PVCs, NetworkPolicies, resource requests/limits and probes. Begin with fixed workers; qualify scaling and draining before HPA. Use at least five workers for the distributed qualification gate. Keep system and Engine user node pools separate; set budgeted autoscaler bounds and topology placement.

Default rollout scope: Engine on AKS, retain existing Studio/API/PostgreSQL initially **only if** secure API-to-Engine connectivity can be established. If the existing API environment cannot reach private AKS services, add reviewed private connectivity or move API to AKS in the same staging phase. Studio migration is optional; PostgreSQL remains managed. API migration requires a controlled schema job and validated session/job behavior before scaling replicas.

## Required operational defaults

- **Identity:** Entra-backed cluster access and least-privilege RBAC; separate deploy, kubelet image-pull and workload identities. Prefer GitHub OIDC over stored Azure client secrets. Enable AKS OIDC issuer and Workload Identity; prove the Engine's actual ADLS credential path supports projected federation before calling it passwordless. Scope Storage Blob Data Reader to the selected read-only dataset container; grant write separately only where needed. See [Microsoft Workload Identity deployment guidance](https://learn.microsoft.com/en-us/azure/aks/workload-identity-deploy-cluster).
- **Network/TLS:** private control plane with a reachable runner/admin path, planned nonoverlapping node/pod/service address space, controlled egress and default-deny workload policies with explicit DNS, identity, storage and service exceptions. Prefer native Engine TLS; otherwise use an isolated TLS proxy and enforce that boundary. Keep workers/exchange/admin routes private. Plan public DNS/certificates only for approved user-facing endpoints. These choices follow the [Microsoft AKS baseline](https://learn.microsoft.com/en-us/azure/architecture/reference-architectures/containers/aks/baseline-aks).
- **Application auth/secrets:** disable `KAVEON_INSECURE_DEVELOPMENT`, Studio local mode and development identities. Supply distinct principal, bridge, catalog-admin and exchange tokens; configure `KAVEON_SECURITY_JSON`, certificate paths or the enforced `KAVEON_TLS_PROXY_BOUNDARY`. `KAVEON_ENGINE_PRIVATE_HTTP=true` is acceptable only inside an explicitly protected boundary. Store secrets in Key Vault and inject through a reviewed secret integration; never put plaintext values in Helm files. Preserve `KAVEON_CREDENTIAL_KEYS` and `KAVEON_CREDENTIAL_ACTIVE_KEY`, retain old keys while ciphertext depends on them, and test rotation. Confirm secret rotation/restart behavior rather than assuming live reload.
- **Durability:** use an Azure Disk CSI PVC for the single coordinator's SQLite catalog/WAL and a sized persistent exchange spool, with `KAVEON_COORDINATOR_EXCHANGE_SPOOL=true`, `KAVEON_EXCHANGE_SPOOL_ROOT` on that volume, and an explicit `KAVEON_EXCHANGE_DISK_LIMIT_BYTES`. Bound worker scratch/spill with ephemeral-storage requests/limits. Do not use an unqualified shared filesystem for SQLite. Define disk retention, consistency-aware backups and tested restores; PVC survival alone is not a backup. Dataset reads should use qualified object storage, not workstation bind mounts. See [Microsoft AKS storage concepts](https://learn.microsoft.com/en-us/azure/aks/concepts-storage).
- **Availability and limits:** coordinator maintenance can interrupt queries; schedule a drain window and preserve PVCs. Set CPU/memory/ephemeral-storage budgets, admission/concurrency limits and timeouts; reserve node headroom. A disruption budget cannot make a single coordinator highly available. Confirm SKU/vCPU, subnet/IP, disk, storage throughput and registry quotas before provisioning.
- **Observability/security:** collect logs and available metrics, redact tokens/SQL-sensitive data, and alert on query errors, worker loss, OOM/restarts, catalog failures, exchange/spill capacity and resource saturation. Configure retention and cost caps. Run non-root where images support it, drop capabilities, use a read-only root filesystem with explicit writable mounts, restrict service-account permissions, pin image digests and scan images. Validate these settings against the real image before enforcement.

## Phases and acceptance gates

1. **Access and design:** select inputs below; read-only inventory resources, permissions, quotas and current configuration. The recorded Kaveon subscription `4ed07f02-b111-4eea-98ce-1c177d573a51` is not visible to the current Azure identity. Do not change context or choose another subscription implicitly.
2. **Reviewable implementation:** author parameterized Bicep and Helm, migration/backup runbooks and an explicit release workflow. Run Bicep build/lint, Helm lint/template and Kubernetes schema validation locally. Once target access and deployment authorization exist, review Bicep what-if and image digests before provisioning staging.
3. **Staging functional/security gate:** validate image pulls, DNS, certificates, federated ADLS reads, secret loading, restart persistence, schema migration, Entra sign-in, Studio/API/Engine exact-result queries, catalog lifecycle, unauthorized/forged-actor rejection and tenant/principal query ownership. Test representative Parquet/Delta/Iceberg paths individually; local format support does not prove every cloud combination.
4. **Distributed resilience/capacity gate:** run the frozen correctness/performance suite on at least five workers; test worker termination, retries, cancellation, duplicate exchange, disk exhaustion, memory pressure, skew, concurrent tenants, coordinator restart, node drain and a sustained soak. Record exact images, machine sizes, dataset, limits, errors and cost. Prove backup/restore in an isolated environment. Do not publish an 8/10 or comparative performance claim without its evidence.
5. **Release and rollback:** admit limited traffic first, observe agreed SLOs, then increase load. Before upgrades, drain queries and back up catalog/PostgreSQL/keyring; retain previous digests and Helm values. Roll back stateless workloads with the prior release only when schema compatibility permits; coordinate data restore for incompatible migrations. Never delete coordinator PVCs during rollback. Keep the existing API endpoint available until routing and rollback have been exercised.

## Missing user inputs before implementation/deployment

1. Accessible tenant/subscription and resource group, region, and which existing ACR/Key Vault/PostgreSQL/storage resources to reuse.
2. Dataset storage account/container/path and required read/write scope; representative dataset size and formats.
3. Engine-only versus API/Studio migration scope, existing network connectivity, allowed public endpoints, domain/DNS ownership and certificate authority.
4. Budget ceiling, worker/node sizing constraints, expected concurrency/latency, availability target, and acceptable maintenance window, RPO and RTO.
5. Entra app/tenant and role/group ownership, GitHub deployment environment/runner connectivity, and operators responsible for secrets, monitoring and recovery.

Do not request secret values in chat. Collect resource identifiers and route credentials through the approved secret store. Implementation and any later provisioning/deployment remain separate from this planning-only deliverable.
