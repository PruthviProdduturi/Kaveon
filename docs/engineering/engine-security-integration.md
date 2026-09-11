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
worker fragment outputs still materialize bounded batches, and stage dependencies
still wait for completed producer stages. Consumers open immutable IPC spools as
batch operators and decode one producer stream at a time, charging the active
encoded stream and decoded batch excess to query memory. Local and worker CPU
execution runs in blocking tasks so HTTP cancellation remains responsive.

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

## Coordinator exchange placement

The coordinator now hosts query exchanges on private disk by default. Producers
upload immutable checksummed chunks there, and retried consumers fetch them from
the same location even after execution moves to another worker. This removes
worker-local exchange storage as a dependency for retrying tasks. It centralizes
exchange I/O on the coordinator; measure its disk and network throughput before
scaling worker count.

`KAVEON_EXCHANGE_SPOOL_ROOT` chooses the parent temporary directory.
`KAVEON_EXCHANGE_DISK_LIMIT_BYTES` defaults to 10 GiB across active stored and
download-retained chunks; each query is limited to 2 GiB. There are at most 1,024
active exchange identities. Disk quota failures return explicit errors. Chunks
expire after 15 minutes without an upload/read. Terminal query cleanup removes
its entries and rejects late uploads; ongoing downloads retain their file leases
until they finish or disconnect. Coordinator restart loses the in-memory index;
this is worker-failure resilience, not coordinator HA or restart recovery.

At initialization the coordinator reconciles only sibling directories whose
names exactly match `kaveon-exchange-<UUID>` and whose directory or immutable
chunk activity is older than the 15-minute exchange TTL. This removes abandoned
chunks from a prior process without traversing `kaveon-result-*` retention data
or unrelated files. Recently active directories are preserved for another
coordinator using the same parent during local operation; an open directory
that the operating system refuses to remove is left for a later retry. This is
garbage collection, not recovery of an interrupted query.

`KAVEON_COORDINATOR_EXCHANGE_SPOOL=false` restores worker-hosted memory exchange
placement. Even in that mode, consumer retries retain the original exchange
location instead of mistakenly fetching from the new execution worker.

## Platform credential encryption

Configure `KAVEON_CREDENTIAL_KEYS` as a JSON map of key IDs to random Fernet keys
and `KAVEON_CREDENTIAL_ACTIVE_KEY` as the active map key. Store the keyring in
the platform's secret manager, independent of metadata backups. Keys must be
random 32-byte URL-safe base64 values; no tenant IDs, passwords, or fallback
defaults are used. Key IDs contain only letters, digits, underscores or hyphens.

New data-source connection strings are persisted in versioned encrypted
envelopes. Missing or invalid key configuration rejects writes with HTTP 503.
Source resolution requires the keyring: legacy plaintext is atomically replaced
using a compare-and-swap update before being used to connect. A concurrent edit
or failed migration stops resolution. Existing envelopes decrypt using their
recorded key ID; on use they rotate to the active key. Retain older keys until
all records have been migrated and required backups no longer need them. Idle
rows are not migrated until used, and existing connection pools must restart to
apply newly rotated connection credentials.

Role-resolution failures now produce `NoAccess`, and role-aware dependencies
reject unknown roles. Production ignores the development identity bypass.
Run API validation from `api` with
`./venv/Scripts/python.exe -m unittest services.test_credentials middleware.test_auth_security services.test_engine_bridge`.
Credential tests exercise real SQLite CAS updates, tamper detection, migration,
rotation, missing-key failure and concurrent-edit protection.


## Platform token claims and legacy encrypted secrets

The platform API validates Entra token issuer against the configured tenant's
v1/v2 issuer URLs and requires matching `tid`, subject, and expiry. Google tokens
require an accepted Google issuer, subject, expiry, and boolean verified-email
claim. These checks follow [Microsoft's token validation guidance](https://learn.microsoft.com/en-us/entra/identity-platform/access-tokens)
and [Google's ID-token guidance](https://developers.google.com/identity/gsi/web/guides/verify-google-id-token).
The API still identifies users by email; immutable provider-subject identity mapping
and issuer-specific authorization scopes need further work.

Google OAuth client secrets now use the same explicit versioned keyring as data
source credentials. New writes have no derived/default
key fallback. Legacy unversioned ciphertext is deliberately rejected at runtime;
migrate it before rollout. From `api`, configure the new keyring and explicitly
supply the exact original secret in `KAVEON_LEGACY_AI_ENCRYPTION_SECRET`, then run:

```powershell
./venv/Scripts/python.exe -m services.migrate_credentials --scope auth-env --auth-env-path PATH
./venv/Scripts/python.exe -m services.migrate_credentials --scope auth-env --auth-env-path PATH --write
```

The first command validates decryption without writes. Quiesce auth
configuration writes during file migration. Remove the legacy secret after migration and retain required prior
keyring entries through rotation. The migration never guesses a historical
fallback and prints counts or sanitized errors only. No environment or database
migration has been executed as part of this change.


## Operational soak qualification

`engine/qualification/soak.py` starts an isolated authenticated coordinator and
2 or 5 native workers using a copied, hashed binary. It repeatedly checks 100k-row
scan, aggregate, join and paged results against DuckDB, using two principals in a
bounded resource group. Active window cancellation holds the group slot while a
second principal queues, then verifies canceled state and queued-query recovery.
The harness kills one worker during a workload wave, samples process RSS/history/
retained disk files each second, and verifies cleanup after deleting paged results.

```powershell
./engine/qualification/venv/Scripts/python.exe engine/qualification/soak.py --server-bin engine/target/debug/kaveon-server.exe --workers 2 --duration-seconds 120 --output tmp/soak-120s
./engine/qualification/venv/Scripts/python.exe engine/qualification/soak.py --server-bin engine/target/debug/kaveon-server.exe --workers 5 --duration-seconds 600 --output tmp/soak-600s
```

The report records every result checksum/failure, fixture hashes, binary hash,
source revision/diff/tree hashes, cancellation/queue outcomes, and RSS samples.
Default operational thresholds are 256 MiB warm-to-tail RSS growth per process,
2 GiB process peak RSS, at most 110 sampled history records, and zero retained
result/exchange/spill files after cleanup. These are explicit soak thresholds,
not promises that allocator RSS equals the logical query memory limit. Passing a
short soak is bounded recovery/retention evidence, not proof of production
availability or a comparative performance result.

The completed September 5 report at `tmp/soak-two-workers-600s/report.json`
passed all ten checks across 600.343 seconds, 4,231 exact-result queries and 23
cancellation/queue cycles. One worker was killed at 300 seconds; surviving nodes
completed the workload. History reached its 100-record bound and cleanup left no
retained files. Peak process RSS was 71.9 MB. This report applies to binary
`aa0cb28eb2d4185e4076f0de9f7b522a802cbc5c6396b5ef84f6d4f3714b32bf`;
it does not certify later source changes or a five-worker soak.

On September 8, current-source verification passed 89 server tests and 21 API
credential/authentication/bridge tests. A fresh native build passed all six TLS
checks and all eight real-HTTP bridge checks using binary
`12b31a7fb461a9ef5be770432f90f97dcbce300b9d34fa954c2a3d50fb451120`.
Those checks establish focused lifecycle, authorization and transport behavior;
they do not replace a renewed soak after the execution changes or the remaining
identity, authorization-policy and availability gates above.

Platform asynchronous SQL results are now bound to the authenticated submitting
user. Polling and deletion require a currently authorized identity and return
the same 404 for missing jobs, another user's jobs and legacy ownerless entries.
Background completion preserves ownership and cannot recreate a result deleted
while its database call was in flight. Deletion discards the retained job/result;
it does not interrupt the underlying database driver. Five route/threading
regressions cover these boundaries; the focused API suite now passes 26 tests.
