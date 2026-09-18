# Run the full Kaveon stack locally with Docker

Status: **verified 2026-09-17** on Windows 11 with Docker Desktop (WSL 2). The
same Compose file runs on macOS and Linux.

This is the self-contained developer install: Studio, the API and DLM,
PostgreSQL, and the KaveonDB Engine (one coordinator, two workers) on one
machine, with no Azure subscription, no Entra sign-in, and no cloud storage.
Everything listens on `127.0.0.1` only. It is a development profile, not a
production deployment; never publish numbers measured on it.

For the Engine alone without PostgreSQL, API or Studio, see
[Run KaveonDB locally](local-kavedb.md). For the same topology as native
processes when Docker is unavailable, see
[Local cluster](../engineering/local-cluster.md).

## What you get

| Service | URL | Notes |
|---|---|---|
| Studio | http://localhost:3000 | signed in automatically as `developer@localhost` (Admin) |
| API | http://localhost:8082 | FastAPI + DLM; `GET /api/health` |
| Engine coordinator | http://localhost:8081 | `/health`, `/ui`, `/v1/...`; insecure-development security profile |
| Engine workers ×2 | internal only | reached through the coordinator |
| PostgreSQL 17 | localhost:5433 | databases `kaveonmeta` and `kaveon`, role `kaveon` / `kaveon-local-only` |

Local-only development tokens are baked into `docker-compose.yml` defaults
(`kaveon-local-admin-token-not-for-production`, `kaveon-local-catalog-admin`,
`kaveon-local-bridge-token-not-for-production`). Override them in `.env` only
if the machine is shared.

## Prerequisites

- Docker Desktop (Windows: WSL 2 backend enabled) or Docker Engine + Compose v2.
  Give the VM at least 8 GB of memory and 4 CPUs; the 504M-row lake below is
  comfortable with 16 GB and 8 CPUs.
- Git.
- Python 3.11+ with `pyarrow` and `numpy` on the host, only for building the
  local lake once and for the one-time catalog registration.
- About 20 GB of free disk for images, PostgreSQL state and the generated lake.

Install the `kaveon` CLI binary (no Rust toolchain needed):

```powershell
# Windows
irm https://raw.githubusercontent.com/PruthviProdduturi/Kaveon/dev/scripts/install.ps1 | iex
```

```bash
# Linux x64 / macOS arm64
curl -fsSL https://raw.githubusercontent.com/PruthviProdduturi/Kaveon/dev/scripts/install.sh | bash
```

## 1. Clone and configure

```powershell
git clone https://github.com/PruthviProdduturi/Kaveon.git
cd Kaveon
```

Create `.env` at the repository root (it is gitignored). The only value the
stack needs is the host directory to mount at `/data` in every Engine container:

```ini
# Host directory mounted at /data in the Engine containers (relative to the repo root).
KAVEON_DATA_PATH=./tmp/kaveon-events
```

If your network blocks the public package CDNs (some corporate networks and
some home routers reject TLS to `files.pythonhosted.org` or
`registry.npmjs.org`; the symptom is `SSLV3_ALERT_HANDSHAKE_FAILURE` during
`docker compose build`), point the image builds at the mirrors your host `pip`
and `npm` already use. Find them with `pip config list` and
`npm config get registry`, then add:

```ini
KAVEON_PIP_INDEX_URL=https://<your-pypi-mirror>/pypi/simple/
KAVEON_NPM_REGISTRY=https://<your-npm-mirror>/npm/
```

## 2. Build the lake

The repository ships a deterministic 504M-row product-telemetry lake generator.
It writes two Parquet tables and the manifest the registration step consumes;
it is idempotent and needs no database:

```powershell
python scripts\build-kaveon-events-parquet.py build --output tmp\kaveon-events
```

Output (about 13 GB):

```text
tmp/kaveon-events/
├── kaveon-events-singlefile-manifest.json
└── kaveon_product/
    ├── kaveon_events_users/combined-v1.parquet      3,000,000 rows
    └── kaveon_events_enriched/combined-v1.parquet   504,000,000 rows
```

Any other directory of Parquet or Delta tables works the same way if you write
a manifest of the same shape for it; see
[Register an Engine catalog](register-engine-catalog.md).

## 3. Build and start

```powershell
docker compose up -d --build
docker compose ps
```

The first build compiles the Engine (Rust, cargo-chef cached), installs the API
(Python) and builds Studio (Next.js); expect 10–20 minutes on a fast machine.
Later starts are seconds. All six services should report `running` or
`healthy`:

```text
kaveon-api                  Up (healthy)
kaveon-engine-coordinator   Up (healthy)
kaveon-engine-worker-1      Up
kaveon-engine-worker-2      Up
kaveon-postgres             Up (healthy)
kaveon-studio               Up (healthy)
```

Verify the cluster sees both workers:

```powershell
curl.exe -s http://localhost:8081/v1/cluster
```

## 4. Register the lake as the `OpenSource` catalog

One-time, from the host, using the API's virtual environment (`cd api; python
-m venv venv; venv\Scripts\pip install -r requirements.txt` if it does not exist
yet). The script records catalog, schema and table definitions on the
coordinator, runs a distributed `COUNT(*)` against every table and refuses to
proceed on a mismatch, then records the source in `kaveonmeta` so Studio and
SQL Lab can see it.

`KAVEON_LOCAL_LAKE_PATH` is the path **as the containers see it** — `/data` —
not the host path.

```powershell
cd api
$env:KAVEON_ENGINE_URL = "http://127.0.0.1:8081"
$env:KAVEON_ENGINE_CATALOG_TOKEN = "kaveon-local-catalog-admin"
$env:KAVEON_ENGINE_BRIDGE_TOKEN = "kaveon-local-bridge-token-not-for-production"
$env:KAVEON_LOCAL_LAKE_PATH = "/data"
$env:METADATA_DB_TYPE = "postgresql"; $env:METADATA_HOST = "127.0.0.1"; $env:METADATA_PORT = "5433"
$env:METADATA_DATABASE = "kaveonmeta"; $env:METADATA_USER = "kaveon"; $env:METADATA_PASSWORD = "kaveon-local-only"; $env:METADATA_SSLMODE = "disable"
$env:PYTHONPATH = "."
.\venv\Scripts\python.exe ..\scripts\register-curated-catalog.py ..\tmp\kaveon-events\kaveon-events-singlefile-manifest.json
cd ..
```

On macOS/Linux, export the same variables and run `venv/bin/python`. In Git
Bash on Windows, prefix the command with `MSYS_NO_PATHCONV=1`; otherwise MSYS
rewrites `/data` into `C:/Program Files/Git/data` and every scan fails with
`No such file or directory`.

Expected output ends with:

```text
Verified kaveon_product.kaveon_events_users: 3000000
Verified kaveon_product.kaveon_events_enriched: 504000000
{"catalog": "OpenSource", "registered_tables": 2}
```

The registration lives in the `catalog-data` volume and survives restarts and
image rebuilds. Re-running the script is safe; it verifies rather than
duplicates.

## 5. Query it

The coordinator runs the insecure-development profile, so the CLI connects
without a token:

```powershell
kaveon http://localhost:8081/OpenSource/kaveon_product --auth none
```

```sql
SHOW TABLES;
DESCRIBE kaveon_events_enriched;

SELECT surface, COUNT(*) AS events, SUM(sessions) AS sessions, AVG(latency_p75_ms) AS p75
FROM kaveon_events_enriched
GROUP BY surface
ORDER BY events DESC;
```

The query summary under the result reports the two worker nodes, the tasks,
and the 504,000,000 rows the storage readers scanned. Set
`--timeout 10m` for long statements; the client default is 30 seconds and a
timeout on the client does not stop the query on the coordinator. Every
statement also appears at http://localhost:3000/engine and
http://localhost:8081/ui.

Studio: open http://localhost:3000, and SQL Lab lists `OpenSource` as a
source.

## Day to day

```powershell
docker compose up -d                 # start (images already built)
docker compose down                  # stop; keeps PostgreSQL state, the catalog and the lake
docker compose down --volumes        # also deletes PostgreSQL state and the Engine catalog registration
docker compose logs -f engine-coordinator
docker compose build engine-coordinator engine-worker-1 engine-worker-2   # after pulling Engine changes
docker compose build api             # after pulling API/DLM changes
docker compose build studio          # after pulling Studio changes
docker compose up -d                 # recreate whatever was rebuilt
```

The lake directory is bind-mounted, so changing `KAVEON_DATA_PATH` or the
files under it needs only `docker compose up -d`, not a rebuild.

## Resource notes

- The coordinator spools exchange data on its container filesystem.
  `docker-compose.yml` sets `KAVEON_EXCHANGE_DISK_LIMIT_BYTES` to 24 GiB and
  `KAVEON_EXCHANGE_QUERY_DISK_LIMIT_BYTES` to 16 GiB, the same as the AKS
  coordinator; the Engine's 10 GiB / 8 GiB defaults refuse a repartitioned
  exact `COUNT(DISTINCT ...)` over the 504M-row table with HTTP 507. Lower
  them in `.env` on a small disk.
- Memory admission and result-cache settings follow the Engine defaults;
  `GET /v1/cluster` shows the live values.
- `engine/qualification/` has its own Compose project (`kaveon-qualification`)
  with a reference PostgreSQL and Trino for differential tests. It is not part
  of this stack and does not need to be running.

## Boundaries

- Identity is a fixed development user; there is no sign-in, TLS or
  role separation. Do not expose the published ports beyond the machine.
- The Engine reads local Parquet and local Delta here. ADLS Gen2 and Iceberg
  paths are not exercised by this profile.
- Timings on a laptop are for development; the benchmark program runs on the
  matched AKS hardware.
