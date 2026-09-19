# Engine governance: resource groups and the audit ledger

What a principal may use of the coordinator, decided at admission by a
named policy, and a durable record of who did what. Both live on the
coordinator's own store — the configuration file, the state directory —
never in PostgreSQL. Sources: `engine/crates/server/src/resource_groups.rs`,
`engine/crates/core/src/memory.rs` (the admission order),
`engine/crates/server/src/audit.rs`, `engine/crates/server/src/api.rs`.

## Resource groups

A resource group is a named policy. Every statement is admitted through
exactly one group, chosen by the ordered selector list at admission, and
the group's limits are on the statement's record
(`context.resource_group`) and in the refusal when it is refused.

| Key | Default | Bound | What it does |
|---|---|---|---|
| `name` | required | nonempty, trimmed, unique, ≤ 128 characters | The group's identity. A `default` group must exist. |
| `max_memory_bytes` | the whole admission pool | 1 to `KAVEON_MEMORY_ADMISSION_LIMIT_BYTES` | The sum of the group's admitted query pools. A statement whose own pool (`KAVEON_QUERY_MEMORY_LIMIT_BYTES`, or its `query_memory_limit_bytes`) exceeds it can never be admitted through the group and is refused on arrival. |
| `max_concurrent` | required | 1 to 10000 | Statements of the group running at once. |
| `max_queued` | 16 | 0 to 10000 | Statements of the group waiting at once; `KAVEON_MEMORY_ADMISSION_QUEUE` bounds every group together. |
| `max_queue_wait_seconds` | 60 | 1 to 86400 | How long a statement of the group waits before HTTP 429. `KAVEON_MEMORY_ADMISSION_WAIT_SECONDS` and the request's `admission_wait_seconds` can only shorten the wait. |
| `max_local_parallelism` | the node's | ≥ 1 | Caps the statement's `local_parallelism` (aggregator threads per task); the request's own value is lowered to it, and the effective value is on the record's `settings`. |
| `priority` | 1 | 1 to 1000 | The group's weight in the cross-group admission order below. |
| `default_settings` | none | the [per-request settings](settings.md#per-request-settings), validated as a request's own | Applied for every key the request left unset, whether the request used the `settings` object or `SET SESSION`. |

Selectors are evaluated in order; the first that matches names the group;
a statement no selector matches goes to `default`. A selector may carry
any of `principal` (exact), `principal_prefix`, `role` (`reader`,
`analyst`, `admin`) and `client_tag` (the request's `client_tags` contains
it); every matcher given must hold. A selector with no matcher is the
catch-all and must be last: anything after it is unreachable and refused
at validation.

```json
{
  "groups": [
    {"name": "interactive", "max_concurrent": 8, "max_queued": 32, "max_queue_wait_seconds": 30,
     "max_memory_bytes": 3221225472, "priority": 4},
    {"name": "etl", "max_concurrent": 2, "max_queued": 64, "max_queue_wait_seconds": 1800,
     "max_local_parallelism": 2, "priority": 1, "default_settings": {"result_cache": false}},
    {"name": "default", "max_concurrent": 4}
  ],
  "selectors": [
    {"principal_prefix": "svc-", "client_tag": "etl", "group": "etl"},
    {"role": "analyst", "group": "interactive"},
    {"principal": "alice", "group": "interactive"}
  ]
}
```

### Where the configuration comes from

In order of precedence, the first that exists:

1. The file named by `KAVEON_RESOURCE_GROUPS` (JSON, or TOML by the
   `.toml` extension; the shape above at the top level). A runtime
   replacement is written back to this file.
2. `<state dir>/resource-groups.json`, the coordinator's durable copy of
   the last `PUT /v1/admin/resource-groups`. Delete it to return to the
   configuration file's section.
3. The `[resource_groups]` section of the configuration file
   (`[[resource_groups.groups]]`, `[[resource_groups.selectors]]`).
4. `security.resource_groups` in `KAVEON_SECURITY_JSON`, the list the
   Engine accepted before 2026-09-19, translated: `max_running` is
   `max_concurrent`, `queue_timeout_ms` rounds up to
   `max_queue_wait_seconds`, each principal is an exact selector and `*`
   the catch-all; a `default` group is added when the list names none.
   Configuring both this list and the section is refused at start.
5. The built-in configuration: one `default` group with
   `KAVEON_PRINCIPAL_QUERY_LIMIT` (default 4) running slots, the node's
   queue and wait, no memory share, priority 1. `KAVEON_PRINCIPAL_QUERY_LIMIT`
   is kept as the alias of that group's `max_concurrent`; before
   2026-09-19 it was a limit per principal, now it bounds the `default`
   group as a whole, and a principal that needs its own bound gets its own
   group through a selector.

The state directory is `KAVEON_STATE_DIR` (`node.state_dir`), by default
the directory of the catalog store (`/state/catalog` on the qualification
cluster, `/var/lib/kaveon` on the local Compose stack).

`GET /v1/admin/resource-groups` (admin) reports the configuration in
force, its `source` (`environment`, `runtime`, `config_file`,
`legacy_security`, `builtin`), the `store_path` a replacement is written
to, and every group's counters. `PUT /v1/admin/resource-groups` (admin)
replaces every group and selector at once after validation — the
controller's policies first, then the durable copy, then the selectors —
so it takes effect for the next admission decision, waiting statements
included, and survives a restart; an invalid document is HTTP 400
`INVALID_RESOURCE_GROUPS` naming the group or selector and nothing
changes. Studio's Settings → Governance page edits the same document.

### The admission order

A statement needs its group's running slot, its group's memory share and
the node's admission pool. All three are decided by one queue per group in
the coordinator's admission controller, so a statement waits once and its
`admission_wait_ms` is the whole wait.

- **Within a group: strict arrival order.** The head is granted first and
  only when its whole budget fits; nothing behind it is admitted ahead of
  it. One team's statements run in the order they were submitted.
- **Across groups: weighted fair share by admitted bytes.** On every
  release, the groups whose head is *eligible* — a running slot free and
  the head within the group's `max_memory_bytes` — are ranked by
  `admitted_bytes / priority`, lowest first, ties to the older head. The
  group furthest below its weighted share of the pool is next. Two groups
  of priority 2 and 1 under sustained contention hold two thirds and one
  third of the pool.
- **The pool is held for the group that is next.** When that group's head
  does not fit the pool, nothing from any other group is admitted until it
  does. A group under its share is therefore never starved by smaller
  statements of groups over theirs; every lease is released when its
  statement ends and every budget is at most the pool, so the head fits
  eventually. The cost is that a large head of an under-served group can
  make a small statement of an over-served group wait; the wait is bounded
  by the group's `max_queue_wait_seconds`.
- **A group at its own limit is skipped.** A group whose head would exceed
  its `max_concurrent` or `max_memory_bytes` is not entitled to more and is
  passed over, so it cannot stall the others; it is considered again at
  the next release.
- The decision is a function of the admitted bytes at that moment: no
  clocks, no virtual time, O(groups) per release, reproducible from the
  counters.

An arrival that asked not to wait (`admission_wait_seconds: 0`) is
admitted only when it would be the next grant under the same order, else
refused at once.

### Refusals

HTTP 429 in every case; the body carries `resource_group`,
`admission_wait_ms` (how long the statement waited before the refusal)
and, when the binding limit is the group's, `limit` with the bound:

| `code` | When | `limit` |
|---|---|---|
| `RESOURCE_GROUP_REJECTED` | the statement's pool exceeds `max_memory_bytes` (refused on arrival, no record) | `{"max_memory_bytes": …}` |
| `RESOURCE_GROUP_REJECTED` | the group's queue holds `max_queued` (refused on arrival, no record) | `{"max_queued": …}` |
| `RESOURCE_GROUP_REJECTED` | the group's `max_queue_wait_seconds` expired first (the record stays `FAILED` with the wait) | `{"max_queue_wait_seconds": …}` |
| `MEMORY_ADMISSION_REJECTED` | the node's queue is full, no wait was allowed and the arrival is not next, or the node's wait expired first | absent |

```json
{
  "error": "resource group 'etl' admits at most 1073741824 bytes per statement share; 3221225472 bytes requested",
  "code": "RESOURCE_GROUP_REJECTED",
  "resource_group": "etl",
  "limit": {"max_memory_bytes": 1073741824},
  "admission_wait_ms": 0
}
```

### Counters

`/v1/node` on the coordinator and its entry on `/v1/cluster` carry
`resource_groups`: per group `limit_bytes`, `max_concurrent`,
`max_queued`, `weight`, `running`, `queued` (now), `admitted_bytes`, and
the cumulative `admitted`, `queued_total`, `rejected`, `withdrawn`, with
`wait_ms_p50` and `wait_ms_p95` over the group's last 1024 admissions
(immediate admissions count as zero). `admission` keeps the node-wide
counters. The same counters are on `GET /v1/admin/resource-groups` under
`counters`.

`ANALYZE … WITH (distinct = true)` runs its per-column counts through the
statement path under the submitting principal's group, at most the
group's `max_concurrent` abreast (and never more than four).

## The audit ledger

An append-only record of who did what on the coordinator, kept on the
coordinator's own disk. Writing a line is an in-memory enqueue on the
request's path; a dedicated thread appends each batch to the current
segment and syncs it once, so a clean shutdown (SIGTERM, Ctrl-C: the
listener closes, in-flight requests finish, the ledger drains) loses
nothing and a crash loses at most the batch in flight. The ledger
references the query record by `query_id` and repeats only what an audit
reader needs; the record store keeps the plan, the stages and the
telemetry.

| Setting | Config key | Default | What it does |
|---|---|---|---|
| `KAVEON_AUDIT_DIR` | `audit.dir` | `<state dir>/audit` | The directory of segments and the catalog cursor. |
| `KAVEON_AUDIT_RETENTION_DAYS` | `audit.retention_days` | 90 | A segment is removed once every record in it is older than this; `0` turns the ledger off (`GET /v1/audit` is then 404 `AUDIT_DISABLED`). |
| `KAVEON_AUDIT_SEGMENT_BYTES` | `audit.segment_bytes` | 67108864 (64 MiB) | A segment is closed and a new one started once it reaches this; at least 1 MiB. |

Segments are `audit-<first record ms>-<first seq>.jsonl`, one JSON object
per line, oldest first; a segment is removed by the 30-second cleanup
loop (and at start) once the segment after it began before the retention
cutoff, so the open segment always stays. `seq` increases across
segments and restarts. Workers keep no ledger.

### Record kinds

Every line carries `seq`, `ts_ms` (Unix milliseconds) and `kind`; the
other fields are present when the kind has them.

| `kind` | When | Fields |
|---|---|---|
| `statement.submitted` | a statement reaches admission (after its settings and context are accepted) | `query_id`, `principal`, `role`, `client`, `source`, `client_tags`, `catalog`, `schema`, `statement_sha256` (SHA-256 of the statement text as submitted, after any `SET SESSION` prefix), `statement` (its first 200 characters), `resource_group` |
| `statement.finished` | the statement's response is sent | the submitted fields, `admission_wait_ms`, `elapsed_ms`, `rows` (the whole result, paged or inline), `bytes_scanned` (compressed bytes the scans read across every task), `mode` (`distributed`, `coordinator`, `cache`) |
| `statement.failed` | the statement failed after admission — an execution error, or a wait that expired | the finished fields, `error_code` (`MEMORY_ADMISSION_REJECTED`, `RESOURCE_GROUP_REJECTED`, a catalog statement's code, `ANALYZE_FAILED`, else `EXECUTION_FAILED`), `error` (first 200 characters) |
| `statement.canceled` | `DELETE /v1/query/{id}`, or the client disconnected | the finished fields, `error_code` (`QUERY_CANCELED`, `CLIENT_DISCONNECTED`) |
| `statement.rejected` | refused on arrival, before any record exists: over the group's share, the group's or the node's queue full, no wait allowed | the submitted fields, `error_code`, `error` |
| `catalog.create`, `catalog.update`, `catalog.delete` | a durable catalog, schema or table definition changed, through `/v1/catalog/*` or a catalog statement | `principal` (the actor), `object_type`, `object_id`, `revision_before` (absent for a create), `revision_after` (absent for a delete), `details` (a cascade's counts) |
| `settings.resource_groups` | `PUT /v1/admin/resource-groups` applied | `principal`, `role`, `details` (`groups_before`, `groups_after`, `selectors_before`, `selectors_after`) |
| `settings.cache_cleared` | `DELETE /v1/cache` | `principal`, `role`, `details` (`cleared_entries`, `cleared_bytes`) |
| `auth.unauthorized`, `auth.forbidden` | a request was refused with 401 or 403 by the security layer | `route` (`METHOD /path`), `principal` (the one the request named or resolved to, when any), `error_code` |

Catalog lines come from the catalog store's own `audit_events` table:
every mutation publishes a snapshot, and the publish drains the events
past the ledger's cursor (`catalog.cursor` in the audit directory) into
the ledger, so both mutation paths are covered without a second write in
either. A ledger opened for the first time positions the cursor at the
store's newest event: it starts at its own start.

```json
{"seq":1042,"ts_ms":1789805123456,"kind":"statement.finished","principal":"alice","client":"kaveon-cli/0.3.0","catalog":"lake","schema":"sales","statement_sha256":"9f86d0…","statement":"SELECT country, COUNT(*) FROM orders GROUP BY 1","resource_group":"interactive","admission_wait_ms":0,"elapsed_ms":412,"rows":42,"bytes_scanned":183504211,"mode":"distributed","query_id":"5f0c…"}
```

### Reading it

`GET /v1/audit` (admin) answers `{"records": [...], "next_cursor": seq}`
oldest first. Parameters: `since` and `until` (Unix milliseconds,
`YYYY-MM-DD`, or `YYYY-MM-DDTHH:MM:SS[.fff]Z`), `principal` (exact),
`kind` (comma-separated exact kinds or families: `statement`, `catalog`,
`settings`, `auth`), `query_id`, `limit` (1 to 1000, default 200) and
`cursor` (a `next_cursor` from the previous page). `format=jsonl` streams
every matching record as `application/x-ndjson`, one object per line, a
page at a time, for export; the same filters apply. A read flushes the
ledger first, so a line enqueued before the call is in the answer. An
unparsable time, an unknown kind or another format is 400
`INVALID_AUDIT_QUERY`; any other role is 403.

The platform proxies both under `/api/v1/engine/audit` and
`/api/v1/engine/admin/resource-groups` for Studio's Settings → Governance
page, which filters, pages and exports the same ledger.
