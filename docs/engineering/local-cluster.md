# Local cluster — the AKS topology as native processes

> For development when the qualification cluster is down (it is paused at weekends, see [suspend and resume](aks-suspend-resume.md)) or when Docker is unavailable on the machine. Same environment contract as `docker-compose.yml` and the Helm charts; what runs here is what runs on AKS **minus** TLS, workload identity, ADLS and network policies. Never publish a number measured here.

This native profile is PostgreSQL-free. It exercises the KaveonDB product
authority, Engine catalog, API and DLM paths on local durable files. PostgreSQL
source capture, outbox replay, reconciliation and rollback remain cloud
migration operations and must use the retirement evidence runbook; they are not
silently started by this profile.

## What runs

| Process | Port | Config | Notes |
|---|---|---|---|
| KaveonDB coordinator | 8080 | `infra/local/engine/coordinator.toml` | insecure-development security profile, dev tokens from `scripts/local-cluster.ps1` |
| KaveonDB workers ×3 | 8081–8083 | `infra/local/engine/worker-N.toml` | discover the coordinator at 127.0.0.1:8080 |
| API | 8090 | environment set by the script | `uvicorn main:app`, dev identity `developer@localhost` as Admin |
| Studio | 3002 | environment in the shell | `node node_modules/next/dist/bin/next dev -p 3002` (corepack cannot fetch pnpm on the home network) |

The lake is a directory. The default is `tmp/kaveon-events`, the output of
`scripts/build-kaveon-events-parquet.py build`, registered as the `OpenSource`
catalog with the same script that registers it on AKS:

```powershell
# once
cd engine; cargo build --release -p kaveon-server -p kaveon-cli; cd ..
python scripts\build-kaveon-events-parquet.py build --output tmp\kaveon-events

# every time
.\scripts\local-cluster.ps1 start
.\scripts\local-cluster.ps1 status
```

Register the lake (runs the same `COUNT(*)` verification as AKS):

```powershell
$env:KAVEON_ENGINE_URL = "http://127.0.0.1:8080"
$env:KAVEON_ENGINE_CATALOG_TOKEN = "kaveon-local-catalog-admin"
$env:KAVEON_ENGINE_BRIDGE_TOKEN = "kaveon-local-bridge-token-not-for-production"
$env:KAVEON_LOCAL_LAKE_PATH = "D:\Repos\PruthviProdduturi\Kaveon\tmp\kaveon-events"
python scripts\register-curated-catalog.py tmp\kaveon-events\kaveon-events-singlefile-manifest.json
```

Studio:

```powershell
cd studio
$env:API_URL = "http://127.0.0.1:8090"; $env:KAVEON_PROXY_SECRET = "kaveon-local-proxy"
$env:AUTH_SECRET = "local-dev-console-preview-secret-0123456789"; $env:AUTH_URL = "http://localhost:3002"; $env:AUTH_TRUST_HOST = "true"
$env:KAVEON_DEV_USER_EMAIL = "developer@localhost"; $env:KAVEON_DEV_USER_ROLE = "Admin"; $env:KAVEON_INSECURE_DEVELOPMENT = "true"
node node_modules\next\dist\bin\next dev -p 3002
```

Then the dataset and the corpus, exactly as on AKS but through the local portal:

```powershell
python scripts\register-kaveon-events-dataset.py --portal http://localhost:3002 --apply --generate
python scripts\qualify-dlm-questions.py --portal http://localhost:3002 --execute-live
```

`register-kaveon-events-dataset.py` and `qualify-dlm-questions.py` sign in with
an Entra token on AKS; against the local portal the dev identity is already
signed in, so both scripts skip the token exchange when `/api/auth/entra-config`
reports Entra disabled.

## What it is good for

- DLM, Studio and API development against a real distributed Engine with the
  504M-row table (the whole corpus runs locally).
- Rehearsing the Monday restore of the qualification cluster without spending
  cluster hours.
- The Engine's local-disk read path, which AKS never exercises.

## What it is not

- Not a performance environment: one machine's cores are shared by four Engine
  processes, the API and Studio, and the lake is a local SSD.
- Not a security environment: the `KAVEON_INSECURE_DEVELOPMENT` profile, dev
  tokens, plain HTTP, no workload identity, no network policies.
- Not ADLS: conditional writes, footer caching over the network, and the
  product-transaction store on ADLS are not exercised.
- Not retirement evidence: a local filesystem checkpoint is accepted only when
  `KAVEON_ENVIRONMENT=local`; it cannot be reused as an AKS cutover receipt.

Verify the local retirement boundary after starting the profile:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\verify-local-postgresql-retirement.ps1
```

The verifier requires a healthy KaveonDB authority and fails if the Windows
PostgreSQL service or either local PostgreSQL port is still active.

## Findings recorded from the first local run (2026-09-13)

- A Parquet file whose Arrow schema stores dictionary-typed text columns
  (pyarrow's and pandas' default, `store_schema=True`) reads as
  `Dictionary(Int32, Utf8)` and the Engine rejects it as a `GROUP BY` key.
  A customer's own Parquet would hit this; `REQUEST @Codex` in HANDSHAKE.
