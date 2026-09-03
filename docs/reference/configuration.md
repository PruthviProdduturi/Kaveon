# Configuration Reference

Copy `.env.example` to `.env` for local development. FastAPI loads the repository-
root `.env`; Studio reads standard Next.js/Auth.js environment variables. Never
commit `.env`, provider secrets, connection strings, or deployment credentials.

## FastAPI settings

These settings are declared in `api/config.py`.

| Variable | Default | Purpose |
|---|---|---|
| `API_PORT` | `8080` | Local API listening port |
| `NODE_ENV` | `development` | Environment label |
| `WEB_URL` | `http://localhost:3000` | Allowed Studio origin used in API CORS headers |
| `AZURE_TENANT_ID` | empty | Azure identity tenant |
| `AZURE_CLIENT_ID` | empty | Azure identity client or managed-identity client ID |
| `KAVEON_PROXY_SECRET` | empty | Enables trust of Studio's proxy identity headers when values match |
| `KAVEON_DEV_USER_EMAIL` | empty | Local API authentication bypass; never set in production |
| `KAVEON_DEV_USER_NAME` | `Dev User` | Name for the local bypass identity |
| `KAVEON_DEV_USER_ROLE` | `Admin` | Role for the local bypass identity |
| `METADATA_DB_TYPE` | empty | `fabric_sql`, `azure_sql`, `postgresql`, or `mysql` |
| `METADATA_ENDPOINT` | empty | Fabric/Azure SQL endpoint |
| `METADATA_DATABASE` | empty | Metadata database name |
| `METADATA_HOST` | empty | PostgreSQL/MySQL host |
| `METADATA_PORT` | `0` | Database port; zero selects the driver default |
| `METADATA_USER` | empty | PostgreSQL/MySQL password-auth user |
| `METADATA_PASSWORD` | empty | PostgreSQL/MySQL password-auth password |
| `METADATA_SSLMODE` | `require` | PostgreSQL SSL mode |
| `DATAWAREHOUSE_ENDPOINT` | empty | Optional default warehouse endpoint |
| `DATAWAREHOUSE_DATABASE` | empty | Optional default warehouse database |
| `AI_ENCRYPTION_SECRET` | empty | Key material for stored AI-provider keys; set explicitly in production |
| `SQL_TRUST_SERVER_CERT` | `true` | ODBC certificate trust override; production guidance is `false` with a trusted certificate |
| `MAX_POOL_SIZE_METADATA` | `10` | Per-process metadata pool size |
| `MAX_POOL_SIZE_DATAWAREHOUSE` | `5` | Per-process warehouse pool size |
| `CONNECTION_TIMEOUT_METADATA` | `30` | Metadata connection timeout in seconds |
| `CONNECTION_TIMEOUT_DATAWAREHOUSE` | `60` | Warehouse connection timeout in seconds |

Additional runtime variables used outside `api/config.py` include:

| Variable | Purpose |
|---|---|
| `AAD_DATABASES` | Comma-separated database names that use Azure token authentication |
| `REDIS_URL` | Enables cross-worker SQL rate limiting; without it the limiter is process-local |

Consult `.env.example` for optional AI-provider and operational settings that may
change independently of the central settings class.

## Studio and Auth.js

| Variable | Required when | Purpose |
|---|---|---|
| `API_URL` | Hosted Studio/API split | Server-side target for `/api/kaveon/*` |
| `KAVEON_PROXY_SECRET` | Proxy identity mode | Must match the API value |
| `AUTH_SECRET` | Hosted OAuth | Signs/encrypts Auth.js session material |
| `AUTH_URL` | Hosted OAuth | Canonical Studio URL |
| `KAVEON_LOCAL_MODE` | Root Compose only | Enables the configured development identity in the loopback-bound local stack; never enable in a hosted deployment |
| `AUTH_ADMIN_EMAILS` | Optional | Comma-separated emails assigned Admin; other signed-in users are Viewer |
| `GITHUB_ID`, `GITHUB_SECRET` | GitHub provider enabled | GitHub OAuth credentials |
| `GOOGLE_ID`, `GOOGLE_SECRET` | Google provider enabled | Google OAuth credentials |
| `AUTH_MICROSOFT_ENTRA_ID_ID`, `AUTH_MICROSOFT_ENTRA_ID_SECRET`, `AUTH_MICROSOFT_ENTRA_ID_ISSUER` | Entra provider enabled | Microsoft Entra OAuth credentials and issuer |

Configure at least one OAuth provider for Studio sign-in. There is no built-in
username/password login.

## Engine configuration — alpha

The CLI accepts `--data-dir` for a directory of local Parquet files and immediate
child Delta table directories. The server
uses TOML configuration; examples are in `engine/kaveon.example.toml` and
`engine/etc/`. Catalog examples are in `engine/catalogs.example/`.

Local Parquet and local Delta Lake storage are executable. Delta support replays
all JSON commits from version 0 and reads the active Parquet files; checkpoint-only
or otherwise incomplete JSON history is unsupported. ADLS Gen2, S3, Iceberg, the
`Optimized` access pattern, and distributed execution remain target capabilities.

## Deployment secrets

GitHub workflows additionally expect Azure service-principal/subscription secrets
for the API deployment and `VERCEL_TOKEN`, `VERCEL_ORG_ID`, and
`VERCEL_PROJECT_ID` for Studio deployment. Those values belong in the repository's
secret store, not `.env` or Bicep parameter files committed to source control.
