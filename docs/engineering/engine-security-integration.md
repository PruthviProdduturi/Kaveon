# Engine security and platform bridge

The Engine now denies unauthenticated API requests by default. Only `/health`
and `/ready` are public. Configure independent credentials for users, the API
bridge, catalog administration, and internal workers. Supply credentials through
your process secret manager; do not commit real tokens to configuration files.

## Server configuration

`KAVEON_SECURITY_JSON` is a JSON object with these fields:

```json
{
  "principals": [
    {"token": "REPLACE_WITH_RANDOM_SECRET_AT_LEAST_32_BYTES", "principal": "alice", "role": "analyst"}
  ],
  "bridge_token": "REPLACE_WITH_DISTINCT_RANDOM_SECRET_AT_LEAST_32_BYTES",
  "resource_groups": [
    {"name": "interactive", "principals": ["alice"], "max_running": 2, "max_queued": 4, "queue_timeout_ms": 30000}
  ]
}
```

User roles are `reader`, `analyst`, and `admin`. Readers can read metadata and
their own query history, but cannot submit SQL. Analysts can submit SQL and read
or cancel their own queries. Administrators can inspect/cancel all query records.
SQL request bodies and forwarded identity headers cannot override a static
token's configured identity. Metadata mutations retain their separate
`KAVEON_CATALOG_ADMIN_TOKEN` plus `x-kaveon-actor` service contract.

The bridge token alone enables trusted `x-kaveon-principal` and
`x-kaveon-role` delegation. Keep it within the authenticated platform API.
Never expose it to Studio/browser clients. The Engine trusts the bridge to
authenticate the actor and resolve roles correctly.

`KAVEON_EXCHANGE_TOKEN` protects worker tasks, exchanges, heartbeats, terminal
cleanup and worker cancellation. All nodes must use the same internal token.

`KAVEON_PRINCIPAL_QUERY_LIMIT` defaults to 4 concurrent statement requests per
principal. Exceeding it returns HTTP 429; permits release on completion/failure.
Each explicitly configured resource group additionally has a FIFO semaphore,
a bounded waiting queue and timeout. The principal request limit counts waiting
requests too. Queue overflow/timeout returns HTTP 429; canceled waiters release
queue slots. Unlisted principals have only the principal and memory limits.
Process memory admission remains an additional independent limit. Weighted
scheduling, CPU quotas and tenant-specific memory budgets remain pending.

## Transport boundary

The listener defaults to `127.0.0.1`. `KAVEON_BIND_HOST` accepts an IP address.
Set `KAVEON_TLS_CERT_PATH` and `KAVEON_TLS_KEY_PATH` to PEM files to enable the
native Rustls HTTPS listener. Both are required; invalid certificates or keys
fail startup. Use certificates issued by a CA trusted by every client and worker.
Certificate renewal currently requires restarting the process.

External plaintext binding instead requires `KAVEON_TLS_PROXY_BOUNDARY=true` and a
TLS-terminating proxy with network isolation that prevents access to the plain
HTTP Engine port. This flag records the operator's assertion; it does not
configure or verify firewall rules. Worker links need the same isolated private
network or TLS proxy boundary. Automated certificate issuance/rotation is pending.

`KAVEON_INSECURE_DEVELOPMENT=true` explicitly enables the legacy anonymous admin
identity for local qualification. It also permits external HTTP binding. Do not
use that switch for production. Invalid supplied tokens still fail even in this
mode. Internal APIs always require the exchange token.

## Platform API configuration and calls

Set `KAVEON_ENGINE_URL` to the coordinator HTTPS endpoint,
`KAVEON_ENGINE_BRIDGE_TOKEN` to the Engine bridge token, and
`KAVEON_ENGINE_CATALOG_TOKEN` to its catalog admin token. HTTPS certificate
verification remains enabled and HTTP redirects are not followed. Loopback HTTP
is supported locally. Private HTTP elsewhere requires an explicit
`KAVEON_ENGINE_PRIVATE_HTTP=true` assertion and actual network isolation.

* `POST /api/v1/sql/engine` accepts the existing SQL execute body. `database`
  selects the Engine catalog. The API requires an authenticated Analyst/Admin,
  validates read-only SQL, and delegates its verified `UserContext` identity.
  The result includes Engine `query_id`, columns, rows and elapsed time.
* `POST /api/v1/catalog-sources/{id}/engine-sync` requires Admin and accepts
  `{}` for first registration or `{"expected_revision": 3}` for changes.
  It derives stable native IDs as `platform-{source UUID}`, passes only location
  and indirect credential fields, and uses Engine `If-Match` compare-and-swap.
  It returns the native catalog and whether it changed. Identical retries do
  not create a new revision. Stale differing updates return HTTP 409.

Source CRUD does not silently synchronize. Synchronize draft/active registration
first and each subsequent lifecycle transition in order. Failed synchronization
leaves platform metadata unchanged and must be retried after resolving the
reported conflict. Source edits and synchronization are not a distributed
transaction. Native catalog lifecycle does not imply schemas or tables have
been registered. External adapters, format/table discovery, background outbox
delivery, Studio UI selection and automatic routing remain pending. The existing
`/sql/execute` database path remains unchanged.

## Result and IPC retention

`POST /v1/statement` accepts `result_delivery: "paged"`. Its response provides
`next_uri`; fetch each relative URI with the same bearer identity. Pages are
immutable/replayable and owner-scoped. Each contains up to 1,000 rows or 4 MiB,
with a 4 MiB maximum row. Per-query disk retention is 256 MiB; process retention
is 1 GiB and 100 results. TTL is 15 minutes. Results disappear on process restart;
cleanup runs every 30 seconds. Unix spool directories are private (0700).
Normal lifecycle/drop removes spool files; abrupt process termination can leave
orphaned temporary files that require host temporary-directory housekeeping.

Local operators and distributed root IPC readers emit batches directly into the
page writer, avoiding a whole-query Arrow/JSON result collection. Inline local
and general distributed results are capped at 16 MiB of Arrow buffers and advise
clients to request paging. Query history retains only 100 rows/64 KiB per result
and bounds terminal record retention. This preview is not the full query result.

Task and exchange HTTP responses are consumed incrementally into private IPC
spools, capped at 128 MiB per response and 512 MiB per process. Worker task reply
caches have a separate 512 MiB process cap; replies stream in 64 KiB pieces,
retaining the cache lease until a slow consumer finishes or disconnects. Exchange
downloads stream validated shared chunks and cap retained active-download bytes
at 512 MiB in addition to the existing store cap. Encoding aborts at its byte
limit. No exchange wire-version change is required.

This bounds network/result retention but does not make operators fully pipelined:
worker fragment execution and input exchange decoding still materialize bounded
batches, and stage dependencies still wait for completed producer stages.

## Validation and remaining gates

Run `cargo test -p kaveon-server security` from `engine` and
`./venv/Scripts/python.exe -m unittest services.test_engine_bridge` from `api`.
Run `api/venv/Scripts/python.exe scripts/qualify-engine-tls.py` from the repository
root to generate ephemeral certificates and verify trusted TLS/authentication while
rejecting unknown certificate authorities and plaintext requests.

These cover missing credentials, forged identity headers, delegated role checks,
query ownership checks, principal quota release, stable mapping, idempotency,
revision conflict behavior and transport rejection. Qualification should also
exercise these boundaries over HTTP with independent user credentials.

These changes provide a credential-based security boundary, not production
identity federation. Entra/OIDC JWT validation at the Engine, credential rotation
without restart, per-catalog/table grants, row/column policies,
durable query audit and fair workload scheduling remain explicit gates.
