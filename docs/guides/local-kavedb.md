# Run KaveonDB locally

The standalone local package runs the Engine and its durable KaveonDB product
catalog without PostgreSQL, the API, or Studio. It is intended for local
Engine/product-record development and CLI use. Cloud deployments continue to
use the existing Helm and Bicep configuration.

From the repository root, install Docker Desktop and run:

```powershell
.\scripts\kavedb.ps1 -Build
```

The service is bound to `127.0.0.1:8080`. Check it before connecting:

```powershell
curl.exe http://localhost:8080/health
kaveon --server http://localhost:8080 --auth none
```

This profile enables the Engine transaction API with the same immutable
revision/CAS protocol used by cloud deployments. Product records are stored in
the durable `kaveon-db-products` volume; the catalog metadata is stored in
`kaveon-db-catalog`. The local mode does not require Azure credentials or
PostgreSQL. It is a single-node development profile, so it does not provide
cloud-level replication or multi-node failover.

Verify the transaction capability and identity boundary:

```powershell
curl.exe -H "Authorization: Bearer kaveon-local-admin-token-not-for-production" `
  http://localhost:8080/v1/capabilities
```

On macOS or Linux:

```bash
./scripts/kavedb.sh build
curl -fsS http://localhost:8080/health
kaveon --server http://localhost:8080 --auth none
```

The default read-only data root is `./data`; point it at another Parquet/Delta
directory with `KAVEON_DATA_PATH` (or `-DataPath` on PowerShell). The product
catalog persists in the named `kaveon-db-catalog` volume. Stop the service with
`kavedb.ps1 -Down` or `./scripts/kavedb.sh down`. Removing the volume deletes
local product records and is an explicit Docker operation.

This local profile enables the Engine's development HTTP boundary and binds
only to loopback. Use the existing TLS, Entra, workload identity, Helm, and
Bicep deployment paths for any shared or cloud environment; do not reuse the
local development tokens outside localhost.
