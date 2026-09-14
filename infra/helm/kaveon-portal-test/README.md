# Kaveon portal test add-on

This chart is a test-AKS add-on for the existing Engine in the `kaveon`
namespace. In normal mode it creates one Studio service (`kaveon-portal:3000`),
one API service (`kaveon-api:8080`), and one PostgreSQL StatefulSet
(`kaveon-postgres:5432`) with one `ReadWriteOnce` PVC. PostgreSQL and its init
Job are omitted in verified restart-rehearsal or final-retirement mode. The
chart does not create a namespace, secrets, credentials, Ingress, or Engine
resources.

Every image is required as an immutable `sha256` digest. Supply a values file
with the three image repositories and digests and the ADLS location for the
already registered `OpenSource` catalog. The init Job applies `/app/schema_postgresql.sql`, then upserts the
active native ADLS catalog-source record using `engine_catalog = OpenSource`.

The deployment assumes these existing same-namespace resources:

| Resource | Required keys / purpose |
| --- | --- |
| Secret `kaveon-portal-auth` | `POSTGRES_PASSWORD`, `METADATA_PASSWORD`, `AUTH_SECRET`, `KAVEON_PROXY_SECRET`, `KAVEON_ENGINE_BRIDGE_TOKEN`, `KAVEON_ENGINE_CATALOG_TOKEN`, `KAVEON_CREDENTIAL_ACTIVE_KEY`, `KAVEON_CREDENTIAL_KEYS`, `AUTH_MICROSOFT_ENTRA_ID_ID`, `AUTH_MICROSOFT_ENTRA_ID_ISSUER`, `AUTH_ENTRA_ADMIN_OBJECT_IDS` |
| ConfigMap `kaveon-engine-ca` | `ca.crt`, the CA used to verify `https://kaveon:8080` |

The Auth.js public-client flow uses `KAVEON_ENTRA_PUBLIC_CLIENT=true`; the chart
does not use an Entra client secret. The API uses its dedicated `kaveon-api`
service account, annotated with `api.workloadIdentity.clientId`, and the pod is
fail-closed with `azure.workload.identity/use=true`. `api.keyVaultUrl` supplies
the RBAC-enabled vault URI for opaque source-secret references; never place a
vault credential or connection string in Helm values.
`NODE_ENV=production` is fixed, and no local-mode or developer-user variables
are supplied.

The PostgreSQL image must be a digest-pinned mirror of Debian-based PostgreSQL
17. Its user and database are deliberately fixed to `kaveon` and `kaveonmeta`;
only the password comes from the existing Secret, and `PGDATA` uses a child of
the PVC mount. Each workload maps only its required Secret keys: PostgreSQL and
the init Job receive the database password, the API receives database/proxy/
Engine/credential keys, and Studio receives Auth.js/proxy/Entra keys. The API
verifies the Engine with the mounted private CA and only uses the internal HTTPS
Engine URL.

Example validation does not contact a cluster:

```powershell
helm lint infra/helm/kaveon-portal-test --namespace kaveon `
  --set images.api.repository=registry.example/kaveon-api `
  --set images.api.digest=sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa `
  --set images.studio.repository=registry.example/kaveon-studio `
  --set images.studio.digest=sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb `
  --set images.postgres.repository=registry.example/postgres `
  --set images.postgres.digest=sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc `
  --set seed.adls.account=storageaccount `
  --set api.workloadIdentity.clientId=00000000-0000-0000-0000-000000000000 `
  --set api.keyVaultUrl=https://example.vault.azure.net/

helm template kaveon-portal-test infra/helm/kaveon-portal-test --namespace kaveon `
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

All cutover switches under `api.cutover` default off. Shadow-read and outbox
flags may be enabled family by family while PostgreSQL remains authoritative;
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
