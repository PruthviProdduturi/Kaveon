# AKS test deployment — September 8, 2026

The user authorized provisioning in Microsoft tenant `72f988bf-86f1-41af-91ab-2d7cd011db47`, subscription `L1R_DSEng` (`eaa4a83d-8511-497c-b0bc-40aa5f0deae1`), resource group `test-prproddu-test`. This resumes AKS deployment only; the engine performance goal remains paused. No subscription policies, exemptions, or subscription-level role assignments were changed.

## Provisioned resources

East US hosts `kaveon-test-aks`, a Kubernetes 1.35.7 test cluster with one system node and three worker nodes. Every node uses Standard_D4s_v3 (4 vCPU, 16 GiB). AKS rejected D4s_v5 for this subscription; the deployment uses an allowed SKU without changing policy. The Free control-plane tier and fixed node counts avoid automatic growth. Four running VMs, disks, registry and network/storage usage remain billable; this is not a spending cap.

- ACR: `kvtestegmf6oweugsno.azurecr.io`, Basic, admin credentials disabled.
- ADLS Gen2: `kvtestegmf6oweugsno`, Standard LRS, including the `kavedb` test catalog container; HTTPS only, shared keys and anonymous blob access disabled, firewall default deny with AKS subnet access.
- Network: `kaveon-test-vnet`, dedicated AKS subnet, Azure CNI overlay.
- Identity: Entra/Azure RBAC cluster access, local cluster accounts disabled, OIDC and workload identity enabled. `kaveon-test-reader` has read-only blob access to the test account and contributor access only on the dedicated `product-transactions` container. It cannot write to the bronze, silver, or gold analytics containers. Caller upload permission is scoped to the test account; image pull and cluster administration roles are scoped to their new resources.
- AKS-managed resources are in `MC_test-prproddu-test_kaveon-test-aks_eastus`.

`infra/bicep/environments/aks-test.bicep` is the resource-group template. Supply the operator object ID and verified outbound IP CIDRs. The initial what-if contained only creates in the test group. Two verified workstation egress addresses were allowed at these resources, but direct workstation access still timed out; deployment uses authenticated `az aks command invoke`. Do not widen access or change subscription policy to work around that network limitation.

## Engine deployment

`infra/helm/kaveon-test` renders a single coordinator and three workers in namespace `kaveon`. Each worker is required to occupy a different worker node. The coordinator uses a 32 GiB Azure Disk PVC for SQLite/WAL and exchange state; workers use bounded ephemeral scratch. This is a single-coordinator test system, not high availability.

The final Engine image is pinned as:

```text
kvtestegmf6oweugsno.azurecr.io/kaveon-engine@sha256:afa2190daf26006864c640e87823268e048882eafe63425fac99c228e51e9ebe
```

Pods run as UID/GID 10001 with a read-only root filesystem, dropped capabilities, TLS and distinct principal/catalog/exchange credentials. Services are ClusterIP only; network policy restricts engine ingress to the namespace. Worker readiness uses `/ready` and remains closed until the worker has recovered the coordinator-required catalog identity; separate live queries still prove registration and distributed execution.

`scripts/aks-test-secrets.py` generates test PKI and secrets in a private ignored directory. Server certificates expire after 30 days; replace the TLS Secret and roll pods before expiry. This initial test uses Kubernetes Secrets, not a completed Key Vault rotation integration. Never commit the generated bundle or print credential files. Apply Secret JSON with `kubectl apply --server-side` to avoid copying the CA bundle into a size-limited last-applied annotation.

Render the Helm chart with the immutable image, then apply it in `kaveon`. The
test portal uses ClusterIP services and is reached through a local `3000` port
forward; it is not a public endpoint. Existing application environments were
not migrated.

## Data and validation

`scripts/generate-medallion-fixture.py` creates deterministic synthetic data: 10,000 orders and 100 customers, raw CSV/JSONL in bronze, typed Parquet in silver, daily aggregates in gold. `scripts/medallion-fixture.md` describes NULL cases, schema, hashes and six exact-result checks. This is a functional smoke dataset, not the five-million-row comparative benchmark.

`scripts/aks-test-bundle.py` prepares a private archive for upload through the allowed cluster subnet and catalog/query checks. Upload uses the caller's short-lived Azure token; engine reads use the separate read-only workload identity. Preserve the generated manifest and expected results alongside validation evidence. The bundle must not be committed. Check both the AKS command's `exitCode` and every engine response/result; ARM command completion alone does not prove success.

At 20:15:46 UTC, all six independently calculated SQL result sets passed on
the live cluster with three registered workers. Checks covered order totals,
NULL customers, customer count, a region join, silver daily aggregation and
matching gold aggregates. Unauthenticated catalog access returned HTTP 401.
These queries establish ADLS workload-identity reads and authenticated TLS
worker execution; they do not establish production performance or the pending
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

After deploying an API image containing the Engine-native DLM row-count
backfill, authenticate to the API and run the repeatable coverage gate. Keep the
port-forward process open in a separate terminal:

```powershell
kubectl --context kaveon-test-aks -n kaveon port-forward service/kaveon-api 18000:8080 --address 127.0.0.1

$env:KAVEON_API_ACCESS_TOKEN = (az account get-access-token `
  --resource api://d0ce7c35-cc10-4ae7-b6be-60d002f43059 `
  --query accessToken -o tsv).Trim()
python scripts/verify-aks-dlm-coverage.py `
  --api-url http://127.0.0.1:18000 `
  --report tmp/aks-dlm-row-count-validation.json
Remove-Item Env:KAVEON_API_ACCESS_TOKEN
```

The request triggers the lazy backfill through the normal authenticated API.
The verifier requires all nine canonical Engine datasets to be `ready` with a
positive exact row count and writes only dataset IDs, names, statuses, counts,
errors and the API URL. It never writes the access token. A failed or missing
dataset makes the command exit nonzero.

Qualify native `ANALYZE` persistence in two phases. The verifier uses the same
authenticated API bridge and stores no bearer token, storage path, manifest, or
credential. Phase one analyzes the known 34-row leaderboard and records only
the Engine query ID/state and bounded statistics diagnostics:

```powershell
python scripts/verify-aks-native-analyze.py --api-url http://127.0.0.1:18000 `
  --phase analyze --report tmp/aks-native-analyze-before-restart.json

kubectl --context kaveon-test-aks -n kaveon delete pod kaveon-coordinator-0
kubectl --context kaveon-test-aks -n kaveon wait --for=condition=Ready `
  pod/kaveon-coordinator-0 --timeout=300s

python scripts/verify-aks-native-analyze.py --api-url http://127.0.0.1:18000 `
  --phase restart --report tmp/aks-native-analyze-after-restart.json
Remove-Item Env:KAVEON_API_ACCESS_TOKEN
```

The restart phase fails unless `OpenSource.ai_benchmarks.leaderboard` still has
34 rows and its current catalog and storage identities match the durable ADLS
record. Join-plan usage remains unqualified until a stable showcase join pair
with independently known cardinalities is designated; the report marks that
check as deferred instead of implying optimizer evidence.

This gate passed on September 10. Native `ANALYZE` query
`c456d1f2-a28d-41e5-9353-475a361fbea6` published a current 34-row statistic.
After a normal coordinator pod deletion and recreation, the statistic retained
catalog digest prefix `df2b742a8246`, source digest prefix `621c2808d6ba`, and
row count 34. All nine canonical DLMs also remained `ready` with positive
`kaveon_engine_exact` counts. Fresh query
`493532d7-cfb0-407d-adc5-2d935d42e34b` then returned 34 through two stages and
all three compatible workers on catalog identity
`sha256:094c016d7dda2b16a10a48e9f543ec5e79368e44553b567ce20fd53094645cb2`.
The Azure Disk CSI driver briefly retried the coordinator mount because
`/dev/sdc` was still in use; the same pod became Ready three seconds later, so
this was an observed detach/mount timing race rather than evidence of data loss.

## Worker-loss and bounded-pressure qualification

Run the credential-safe live reliability gate only when no rollout or other
cluster mutation is active. The verifier reads the static Engine principal and
CA from the existing Kubernetes Secrets into memory, validates the cluster DNS
name over a loopback-only port-forward, and never writes a token,
certificate, or storage credential to the report:

```powershell
python scripts/qualify-aks-fault-pressure.py `
  --context kaveon-test-aks --namespace kaveon `
  --concurrency 4 --concurrent-rounds 3 --pressure-rounds 3 `
  --output tmp/aks-fault-pressure/report.json
```

The gate first executes 12 concurrent exact-result queries against independently
recorded OpenSource row totals. It then establishes a distributed yellow-taxi
join baseline, force-terminates the StatefulSet worker that owned the baseline's
longest task while the repeated join is `RUNNING`, requires an incremented task
attempt and the exact baseline result, and waits for a replacement pod with the
same required catalog identity. Finally, it runs 12 grouped pressure queries at
concurrency four while sampling pod CPU and memory. Every result hash must match,
unrelated restart counts must remain unchanged, sampled Engine memory must stay
within pod limits, and retained spill/exchange file counts must not grow.

The report keeps pre-existing retained-file hygiene as a separate fail-closed
check. A stable nonzero count proves that this workload added no files, but it
does not prove restart cleanup or leak-free operation. Do not delete old exchange
files merely to make the check pass; identify and correct their lifecycle first.

On September 10, Engine digest
`sha256:bd5a6ef6cdb00a121f215947c98a948f5e3fbd9d2c4b094a7d946d07a71d2bc6`
passed the workload-specific gates. All 12 concurrent known-result queries and
all 12 pressure queries returned exact HTTP 200 results. While the repeated
four-stage taxi join was running, `kaveon-worker-2` was force-terminated; stage
2 partition 2 retried as attempt 1 on `kaveon-worker-0` and returned the exact
baseline `[[3412043, 6077935]]`. The replacement worker restored three active
and compatible workers with the same catalog identity. No unrelated container
restart or retained-file growth occurred, and sampled Engine memory stayed
within pod limits; peak coordinator/worker memory was 70/92/135/9 MiB.

The overall fail-closed report remains unsuccessful because eight coordinator
exchange chunks from September 9 and early September 10 survived the prior
coordinator restarts. This run left that count unchanged at eight and left zero
worker spill files. The current workload therefore closes the worker retry,
concurrent exact-result, and bounded-pressure execution gaps, while coordinator
restart cleanup remains open. The credential-free machine-readable report is
retained in `docs/engineering/engine-aks-fault-pressure-validation-2026-09-10.json`;
its SHA-256 is `90d74c9eff118804942c881fdff4de931dcdd164f2730c25de00284d8596184c`.

## Stop and resume

PostgreSQL is still the authoritative Studio metadata store. Follow
`docs/engineering/postgresql-retirement.md`; do not remove its StatefulSet or
PVC until every migration, cutover, restart, and rollback gate there passes.
The API pod has a fail-closed init check for the non-null
`product_migration_outbox.owner_principal` schema contract. During an upgrade,
the API cannot accept product writes until the additive schema job has applied
that contract. On initial install the schema Job runs alongside the waiting API;
on upgrades it is a fail-fast `pre-upgrade` hook, removed after success so its
immutable Job spec cannot block the next release. This ordering barrier does
not enable migration or replace the retirement gates.

The latest qualified rollout has all four Engine pods Ready on
`kvtestegmf6oweugsno.azurecr.io/kaveon-engine@sha256:bd5a6ef6cdb00a121f215947c98a948f5e3fbd9d2c4b094a7d946d07a71d2bc6`.
Studio is Ready on
`kvtestegmf6oweugsno.azurecr.io/kaveon-studio@sha256:7a6b11b3859506cee65cffbfe234523101fabf53c1c5f7d7b9a065f018fc51b7`;
the API is Ready on
`kvtestegmf6oweugsno.azurecr.io/kaveon-api@sha256:4dc35a7b61f905f7debbf93f45da68c5560a2a17d6ca33e485d1956f54e526f6`.
The portal is authenticated with a public Entra client and server-verified
session; no subscription policy changed for this rollout. The coordinator
restart preserves catalog data but in-memory query history starts afresh, so
attribution checks use newly submitted queries.

The `kavedb` catalog contains `bronze.orders`, `bronze.customers`,
`silver.orders`, `silver.customers`, and `gold.daily_sales` in ADLS Parquet.
A live CLI query returned COUNT 10000 and SUM(amount_cents) 486727696,
with 10000 scanned rows and 59186 selected compressed bytes. A real Microsoft
token established an Admin portal session; an Edge browser then selected
`kavedb` / `silver` and ran the same exact aggregate through deployed SQL Lab.
The browser test used real API, Engine, and storage responses. Interactive
Microsoft popup/MFA completion remains a manual check.

To stop compute between sessions (after queries finish):

```powershell
az aks stop --subscription eaa4a83d-8511-497c-b0bc-40aa5f0deae1 --resource-group test-prproddu-test --name kaveon-test-aks
```

Resume with the same command using `start`. Storage, registry, disks and some networking charges persist while stopped. Resource group deletion is destructive and is not part of this deployment. The cluster is left running for the requested testing unless the user asks otherwise.
