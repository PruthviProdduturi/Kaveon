# Operations and Troubleshooting

## Health and readiness

| Component | Check | Meaning |
|---|---|---|
| Studio | Open the configured Studio URL | Next.js is reachable; this alone does not prove API/database health |
| FastAPI | `GET /api/health` | API process health |
| Engine | `GET /health` | Engine process health and version |
| Engine | `GET /ready` | At least one Engine catalog is loaded |

## Common failures

### Studio returns proxy or authentication errors

Confirm `API_URL` points to FastAPI, both services use the same
`KAVEON_PROXY_SECRET`, `AUTH_URL` matches Studio's public URL, and the OAuth callback
matches the provider configuration. Do not place the proxy secret in browser code.

### Browser requests fail CORS

Set `WEB_URL` to the exact Studio origin, including scheme and without an unrelated
path. FastAPI emits that single configured origin. CORS does not replace API ingress
restrictions or authentication.

### Metadata database is unavailable

Verify the `METADATA_*` settings, DNS/network access, TLS mode, and database role.
For passwordless Azure PostgreSQL, verify the Container App identity was created as
a database principal and has privileges. For Fabric/Azure SQL, verify ODBC Driver 18
and Azure credential availability.

### A registered source “tests” successfully but queries fail

The data-source test endpoint currently returns a stub success message. Validate
with SQL Lab (`SELECT 1`) or the applicable setup/admin probe, then inspect API logs.

### Engine ignores `ORDER BY`

This is expected in alpha: SQL parsing creates a Sort plan, but the physical
planners pass it through. Do not rely on result ordering until Sort/TopN operators
are implemented.

### Engine workers appear but queries do not scale out

Heartbeats implement discovery only. Statements execute on the receiving
coordinator; fragment scheduling, exchanges, shuffle, and retry are target work.

### Queries/jobs disappear after restart

Engine query history and FastAPI detached job/cache state are process-local.
Restarts clear them. Multi-replica durable job coordination is not implemented.

## Backups and recovery

Back up the metadata database using the database provider's supported tools. It
contains datasets, charts, dashboards, user state, history, and DLM artifacts.
Customer analytical databases require their own backup policy. The repository does
not provide a restore orchestrator; rehearse database restore and configuration
secret recovery for each deployment.

## Safe escalation data

Record the commit/version, component, route or SQL, timestamp/time zone, sanitized
error, database type, deployment topology, and whether the failure reproduces after
a process restart. Never attach `.env`, access tokens, connection strings, or raw
customer rows to a public issue.

See [Deployment](../DEPLOYMENT.md), [Security](../SECURITY.md), and
[configuration](reference/configuration.md).
