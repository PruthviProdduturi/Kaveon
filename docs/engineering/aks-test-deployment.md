# AKS test deployment — September 8, 2026

The user authorized provisioning in Microsoft tenant `72f988bf-86f1-41af-91ab-2d7cd011db47`, subscription `L1R_DSEng` (`eaa4a83d-8511-497c-b0bc-40aa5f0deae1`), resource group `test-prproddu-test`. This resumes AKS deployment only; the engine performance goal remains paused. No subscription policies, exemptions, or subscription-level role assignments were changed.

## Provisioned resources

East US hosts `kaveon-test-aks`, a Kubernetes 1.35.7 test cluster with one system node and three worker nodes. Every node uses Standard_D4s_v3 (4 vCPU, 16 GiB). AKS rejected D4s_v5 for this subscription; the deployment uses an allowed SKU without changing policy. The Free control-plane tier and fixed node counts avoid automatic growth. Four running VMs, disks, registry and network/storage usage remain billable; this is not a spending cap.

- ACR: `kvtestegmf6oweugsno.azurecr.io`, Basic, admin credentials disabled.
- ADLS Gen2: `kvtestegmf6oweugsno`, Standard LRS, containers `bronze`, `silver`, `gold`; HTTPS only, shared keys and anonymous blob access disabled, firewall default deny with AKS subnet access.
- Network: `kaveon-test-vnet`, dedicated AKS subnet, Azure CNI overlay.
- Identity: Entra/Azure RBAC cluster access, local cluster accounts disabled, OIDC and workload identity enabled. `kaveon-test-reader` has read-only blob access to this test account. Caller upload permission is scoped to the test account; image pull and cluster administration roles are scoped to their new resources.
- AKS-managed resources are in `MC_test-prproddu-test_kaveon-test-aks_eastus`.

`infra/bicep/environments/aks-test.bicep` is the resource-group template. Supply the operator object ID and verified outbound IP CIDRs. The initial what-if contained only creates in the test group. Two verified workstation egress addresses were allowed at these resources, but direct workstation access still timed out; deployment uses authenticated `az aks command invoke`. Do not widen access or change subscription policy to work around that network limitation.

## Engine deployment

`infra/helm/kaveon-test` renders a single coordinator and three workers in namespace `kaveon`. Each worker is required to occupy a different worker node. The coordinator uses a 32 GiB Azure Disk PVC for SQLite/WAL and exchange state; workers use bounded ephemeral scratch. This is a single-coordinator test system, not high availability.

The image was rebuilt from engine source at `d567232` and pushed as:

```text
kvtestegmf6oweugsno.azurecr.io/kaveon-engine@sha256:b0fbcc0ba878ea956152b0985f39c1996fc1f9c29b42200b70d6c3852dfc4ea8
```

Pods run as UID/GID 10001 with a read-only root filesystem, dropped capabilities, TLS and distinct principal/catalog/exchange credentials. Services are ClusterIP only; network policy restricts engine ingress to the namespace. Worker readiness checks process health because workers execute shipped fragments without loading the coordinator catalog; separate live queries must prove worker registration and execution.

`scripts/aks-test-secrets.py` generates test PKI and secrets in a private ignored directory. Server certificates expire after 30 days; replace the TLS Secret and roll pods before expiry. This initial test uses Kubernetes Secrets, not a completed Key Vault rotation integration. Never commit the generated bundle or print credential files. Apply Secret JSON with `kubectl apply --server-side` to avoid copying the CA bundle into a size-limited last-applied annotation.

Render the Helm chart with the immutable image and workload identity client ID, then apply it in `kaveon`. No public Studio/API endpoint or managed PostgreSQL was deployed by this engine test rollout. Existing application environments were not migrated.

## Data and validation

`scripts/generate-medallion-fixture.py` creates deterministic synthetic data: 10,000 orders and 100 customers, raw CSV/JSONL in bronze, typed Parquet in silver, daily aggregates in gold. `scripts/medallion-fixture.md` describes NULL cases, schema, hashes and six exact-result checks. This is a functional smoke dataset, not the five-million-row comparative benchmark.

`scripts/aks-test-bundle.py` prepares a private archive for upload through the allowed cluster subnet and catalog/query checks. Upload uses the caller's short-lived Azure token; engine reads use the separate read-only workload identity. Preserve the generated manifest and expected results alongside validation evidence. The bundle must not be committed. Check both the AKS command's `exitCode` and every engine response/result; ARM command completion alone does not prove success.

At 20:15:46 UTC, all six independently calculated SQL result sets passed on
the live cluster with three registered workers. Checks covered order totals,
NULL customers, customer count, a region join, silver daily aggregation and
matching gold aggregates. Unauthenticated catalog access returned HTTP 401.
These queries establish ADLS workload-identity reads and authenticated TLS
worker execution; they do not establish production performance or the paused
Trino comparison target. Raw command output is in
`tmp/aks-initial-validation.json`; `scripts/verify-aks-test-results.py` checks
the output against fixture expectations.

Silver tables are `medallion.test.orders` and `medallion.test.customers`;
gold is `medallion_gold.test.daily_sales`. ADLS catalog locations use paths
relative to the catalog container. The initial absolute-path bootstrap was
corrected with revision checks; its obsolete silver `daily_sales` entry is
suspended. A one-time `--repair-initial-catalog` option preserves that repair
procedure, while fresh deployment uses the corrected definitions directly.

The coordinator was subsequently deleted and recreated normally against the
same PVC. Without catalog bootstrap, all six SQL checks passed again, three
workers registered, and unauthenticated access again returned 401. The final
coordinator and all three workers are Running and Ready on four distinct
nodes. Raw evidence is `tmp/aks-restart-validation.json`; a compact verified
report is checked in at `docs/engineering/aks-test-validation-2026-09-08.json`.
Temporary upload credentials and test payloads were removed from the running
coordinator after validation. This verifies restart persistence, not backup
restore or recovery of interrupted queries.

## Stop and resume

The September 8 CLI/UI follow-up deployed coordinator image
`kvtestegmf6oweugsno.azurecr.io/kaveon-engine@sha256:909cf21c79cc87d5bf4085f7f155aa23e90a9ed00f1ea968c0fea10327e2883a`
from `ad3d9c8`. Workers retained their qualified image. Live query history and a
real Edge browser check show **Kaveon CLI** and the signed Entra username; an
explicit spoofed request username is ignored. Three workers are active. The
coordinator restart preserves catalog data but in-memory query history starts
afresh, so attribution checks use newly submitted queries.

To stop compute between sessions (after queries finish):

```powershell
az aks stop --subscription eaa4a83d-8511-497c-b0bc-40aa5f0deae1 --resource-group test-prproddu-test --name kaveon-test-aks
```

Resume with the same command using `start`. Storage, registry, disks and some networking charges persist while stopped. Resource group deletion is destructive and is not part of this deployment. The cluster is left running for the requested testing unless the user asks otherwise.
