# Kaveon portal test add-on

This chart is a test-AKS add-on for the existing Engine in the `kaveon`
namespace. It creates one Studio service (`kaveon-portal:3000`), one API service
(`kaveon-api:8080`), and one PostgreSQL StatefulSet (`kaveon-postgres:5432`) with
one `ReadWriteOnce` PVC. It does not create a namespace, secrets, credentials,
Ingress, or Engine resources.

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
does not use an Entra client secret, federation, or workload-identity annotation.
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
  --set seed.adls.account=storageaccount

helm template kaveon-portal-test infra/helm/kaveon-portal-test --namespace kaveon `
  --set images.api.repository=registry.example/kaveon-api `
  --set images.api.digest=sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa `
  --set images.studio.repository=registry.example/kaveon-studio `
  --set images.studio.digest=sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb `
  --set images.postgres.repository=registry.example/postgres `
  --set images.postgres.digest=sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc `
  --set seed.adls.account=storageaccount
```

Deploy only after the referenced Secret, CA ConfigMap, and Engine TLS service
have been prepared separately.
