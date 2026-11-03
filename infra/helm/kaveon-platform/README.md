# Kaveon platform Helm chart

This chart is the Kaveon private-cloud release for the existing Engine in the `kaveon`
namespace. The directory name is retained for backwards compatibility. A
native install creates one Studio service (`kaveon-portal:3000`) and one API
service (`kaveon-api:8080`); the chart defaults to PostgreSQL-free
(`api.cutover.retirementMode: true`). The legacy PostgreSQL StatefulSet
(`kaveon-postgres:5432`) and schema-init Job are rendered only when both
retirement and restart-rehearsal modes are disabled. The chart does not create
a namespace, secrets, credentials, Ingress, or Engine resources.

Every rendered image is required as an immutable `sha256` digest. Supply a
values file with the API and Studio image repositories and digests plus the
ADLS location for the
already registered `OpenSource` catalog. In legacy compatibility mode, the init
Job applies `/app/schema_postgresql.sql`; the native path instead upserts the
active ADLS catalog-source record through KaveonDB.

The automated deploy creates the portal Secret and Engine CA ConfigMap; manual Helm installs must provide these same-namespace resources:

| Resource | Required keys / purpose |
| --- | --- |
| Secret `kaveon-portal-auth` | Native mode: `AUTH_SECRET`, `KAVEON_PROXY_SECRET`, `KAVEON_ENGINE_BRIDGE_TOKEN`, `KAVEON_ENGINE_CATALOG_TOKEN`, `KAVEON_CREDENTIAL_ACTIVE_KEY`, `KAVEON_CREDENTIAL_KEYS`, `AUTH_MICROSOFT_ENTRA_ID_ID`, `AUTH_MICROSOFT_ENTRA_ID_ISSUER`, `AUTH_ENTRA_ADMIN_OBJECT_IDS`; legacy PostgreSQL mode additionally requires `POSTGRES_PASSWORD` and `METADATA_PASSWORD` |
| ConfigMap `kaveon-engine-ca` | `ca.crt`, the CA used to verify `https://kaveon:8080` |

The Auth.js public-client flow uses `KAVEON_ENTRA_PUBLIC_CLIENT=true`; the chart
does not use an Entra client secret. The API uses its dedicated `kaveon-api`
service account, annotated with `api.workloadIdentity.clientId`, and the pod is
fail-closed with `azure.workload.identity/use=true`. `api.keyVaultUrl` supplies
the RBAC-enabled vault URI for opaque source-secret references; never place a
vault credential or connection string in Helm values.
`NODE_ENV=production` is fixed, and no local-mode or developer-user variables
are supplied.

`api.demoMode` (default `false`) sets `KAVEON_DEMO_MODE` on the API: the
public demo's read-only posture below the Admin role, with submitted SQL
confined to read statements. The Engine's half of the posture — the
per-principal live-read quota — is the Engine chart's `resourceGroups` block;
set both for a demo cluster and neither for anything else.

If legacy compatibility mode is explicitly selected, the PostgreSQL image must
be a digest-pinned mirror of Debian-based PostgreSQL 17. Its user and database
are deliberately fixed to `kaveon` and `kaveonmeta`; only the password comes
from the existing Secret, and `PGDATA` uses a child of the PVC mount. Native
PostgreSQL-free installs do not render those workloads or require their
credentials; the API receives the KaveonDB, Engine, proxy, and credential keys
it needs, and Studio receives Auth.js/proxy/Entra keys.

Example validation does not contact a cluster:

```powershell
helm lint infra/helm/kaveon-platform --namespace kaveon `
  --set images.api.repository=registry.example/kaveon-api `
  --set images.api.digest=sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa `
  --set images.studio.repository=registry.example/kaveon-studio `
  --set images.studio.digest=sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb `
  --set images.postgres.repository=registry.example/postgres `
  --set images.postgres.digest=sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc `
  --set seed.adls.account=storageaccount `
  --set api.workloadIdentity.clientId=00000000-0000-0000-0000-000000000000 `
  --set api.keyVaultUrl=https://example.vault.azure.net/

helm template kaveon-portal-test infra/helm/kaveon-platform --namespace kaveon `
  --set images.api.repository=registry.example/kaveon-api `
  --set images.api.digest=sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa `
  --set images.studio.repository=registry.example/kaveon-studio `
  --set images.studio.digest=sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb `
  --set images.postgres.repository=registry.example/postgres `
  --set images.postgres.digest=sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc `
  --set seed.adls.account=storageaccount `
  --set api.workloadIdentity.clientId=00000000-0000-0000-0000-000000000000 `
  --set api.keyVaultUrl=https://example.vault.azure.net/
```

Deploy only after the referenced Secret, CA ConfigMap, and Engine TLS service
have been prepared separately.

## PostgreSQL retirement controls

The chart can provision the shared evidence claim with `api.evidenceStorage.create=true`.
On AKS, use an RWX Azure Files CSI storage class so report Jobs can write while
the API mounts the same claim read-only. The claim carries Helm's `keep` policy
by default because uninstalling a rehearsal must not delete its evidence. Set
the same claim name in `cutover.evidencePvc`, `retirementReports.evidencePvc`,
and `postgresqlFreeSmoke.evidencePvc`.

Retirement is enabled by default. Shadow-read and outbox flags may be enabled
family by family only in an explicitly selected rehearsal/legacy overlay while
PostgreSQL remains authoritative;
`productReplay.enabled` controls the replay worker independently. The write
fence is also explicit and should be activated only for the final live drain.

`restartRehearsalMode` and `retirementMode` are mutually exclusive. Enabling
either requires the evidence PVC, exact authority and read-authority family
lists, and compiled-DLM ADLS account/container. The API's PostgreSQL schema init
container is omitted only in those modes. The evidence PVC is mounted read-only
at `/retirement`, and runtime validation must pass before the process clears its
PostgreSQL environment. Rehearsal uses the family-report and operational-receipt
directories; final mode requires the complete evidence and matching audit.

The `dlmRunMigration` Job exposes durable ADLS checkpoint and immutable artifact
locations separately and requires all of them when enabled. The
`retirementReports` Jobs are also disabled by default and require a restricted
PVC containing live observations; they validate and write the two special
reports but never perform deletion or invent observations. Keep these controls
in a reviewed values file rather than long `--set` commands so comma-delimited
family lists remain exact and auditable.

After the PostgreSQL-free API is ready, enable `api.postgresqlFreeSmoke` for one
release revision. The Job requires a migrated fixture identity, a visible ready
DLM dataset ID, a bounded question and the evidence PVC. It reads
`KAVEON_PROXY_SECRET` from `secrets.portalAuth`, uses no service-account token,
and writes the content-free receipt at `outputFile`. The in-cluster API URL uses
plain HTTP by design; for an HTTPS route set its exact hostname in
`allowedHosts`, set `useCaCert=true`, and supply either `caSecret` or
`caConfigMap` plus `caKey`. Preserve the completed Job logs and
receipt, then disable the Job in the next values revision. A failed or missing
Job receipt is not restart-recovery evidence.

Start from the checked-in
[`examples/postgresql-free.values.yaml`](examples/postgresql-free.values.yaml)
for the first PostgreSQL-free boot. It contains the exact 16 evidence families,
the exact 12 runtime read families, ADLS artifact/checkpoint locations, the
restricted evidence PVC and the smoke Job without any credential value. Replace
all angle-bracket placeholders and render it before use:

```powershell
$values = "infra/helm/kaveon-platform/examples/postgresql-free.values.yaml"
helm lint infra/helm/kaveon-platform --namespace kaveon -f $values
helm template kaveon-portal-test infra/helm/kaveon-platform `
  --namespace kaveon -f $values > tmp/postgresql-free-rendered.yaml
```

The example selects restart rehearsal (`true/false`). After its smoke receipt,
restart-recovery receipt and the complete final audit are archived on the PVC,
create a reviewed final overlay that changes only those mode booleans to
`false/true`. Keep the write fence on, replay/shadow/report Jobs off, and the
PostgreSQL image reference available for rollback. Neither mode renders the
PostgreSQL StatefulSet or deletes its PVC.

## Automated AKS deployment

`.github/workflows/deploy-aks-platform.yml` exposes the same script behind an OIDC-authenticated, production-environment workflow. It is fail-closed: missing image digests, Engine CA, workload identity, Key Vault, ADLS artifact locations, auth credentials, or reviewed retirement evidence stop the run before Helm mutation. Use `rehearsal` for the first PostgreSQL-free restart rehearsal and `retirement` only after the complete evidence validator has passed.
