# Self-hosting Kaveon

One command on any machine with Docker:

```bash
git clone https://github.com/PruthviProdduturi/Kaveon.git
cd Kaveon
./scripts/kaveon-up.sh
```

That brings up the Engine coordinator, two Engine workers, the API and Studio,
and prints their addresses. There is no PostgreSQL to install and no database to
provision: KaveonDB is the system of record.

Re-running the script is safe. It owns one block of `.env` and leaves every
other key in that file exactly as you wrote it, secrets included.

## The one decision: where Kaveon keeps its own memory

`--storage` names the store that holds **the platform's own records** —
dashboards, charts, datasets, saved statements, chat history, favourites,
permissions and the audit ledger. It is written transactionally and is the one
location you choose at deployment.

> **Today it is not yet the whole of Kaveon's memory.** The Engine's catalog —
> which catalogs exist, their schemas, every table definition, the planner's
> statistics and the cubes — lives in a SQLite database on the coordinator
> (`/var/lib/kaveon/catalog.db`, the `catalog-data` volume), not in this store.
> Both have to be kept. [System storage](../engineering/system-storage.md) is
> the design that folds the second into the first and names what is still
> outstanding; until it lands, read ["Replacing the
> host"](#replacing-the-host) before you need it.

```bash
./scripts/kaveon-up.sh                                       # local disk
./scripts/kaveon-up.sh --storage s3://my-bucket/kaveon
./scripts/kaveon-up.sh --storage adls://myaccount/system/kaveon
```

| | use it when |
|---|---|
| `file:///path` | a laptop, a single server, an evaluation. The default. |
| `s3://bucket/prefix` | AWS, or anything S3-compatible with conditional writes |
| `adls://account/container/prefix` | Azure |

**Table data is a separate question.** A catalog can point at any location the
deployment can reach — another bucket, another account, a mounted directory,
someone else's Delta or Iceberg tables. The *record* of those catalogs is always
written to the system store. So you can keep the system store on Azure and query
data in S3, or the other way round.

Choose it once. Moving it later is a migration, not a setting — see
[system storage](../engineering/system-storage.md).

Credentials never go in the URL and never go in `.env`. ADLS uses workload
identity, falling back to managed identity; S3 uses the standard provider chain
(environment, web identity, instance profile). The host supplies them; Kaveon
reads them.

## Bringing your data

Mount the directory holding it and register a catalog over it:

```bash
./scripts/kaveon-up.sh --data /mnt/warehouse
```

Then in Studio: **Catalog → Add schema**, register tables by location, and run
`ANALYZE` on each. To register a whole catalog at once — or to rebuild one after
replacing a host — `scripts/register-lake-catalog.py` does the same thing
without the clicking, and skips anything already registered.

A table whose statistics are current answers counts, totals and column bounds
*without reading the data*; one that has never been analyzed is read in full for
every question. The Catalog page says which is which.

Parquet (single file or directory), Delta and Iceberg are read natively. No Hive
metastore, no external catalog service.

## Sign-in

Without an OAuth provider the stack runs in local mode with a single development
identity. That is fine on a laptop and **not fine on a network**.

To require real sign-in, set any one of GitHub, Google or Microsoft Entra in
`.env` and restart:

```bash
AUTH_MICROSOFT_ENTRA_ID_ID=<application id>
AUTH_MICROSOFT_ENTRA_ID_SECRET=<client secret>
AUTH_ADMIN_EMAILS=you@example.com
```

Everyone signing in is a Viewer unless their address is in `AUTH_ADMIN_EMAILS`.
Roles run `Viewer < Analyst < Editor < Admin`; content is `private`, `internal`
or `published`.

The browser never talks to the API directly — Studio proxies every call and
stamps the identity with `KAVEON_PROXY_SECRET`, which the script generates.

## Running it on a server

The stack publishes to loopback only. To serve it to other people, put a reverse
proxy with TLS in front of Studio on :3000 and set `AUTH_URL` and `WEB_URL` to
the public address. Do not publish the Engine (:8081) or the API (:8082) — Studio
is the only front door, and the API trusts the proxy header.

Sizing: two vCPUs and 4 GiB runs the full stack comfortably for a small team.
The Engine's share is set by `KAVEON_QUERY_MEMORY_LIMIT_BYTES` and the admission
queue; see [engine settings](../engine/settings.md).

## Day two

```bash
docker compose ps                  # what is running
docker compose logs -f api         # follow a service
docker compose down                # stop, keep data
./scripts/kaveon-up.sh             # start again; secrets and your own keys kept
```

Back up **both** places Kaveon keeps its memory. The data they point at is
backed up wherever it lives.

| What | Where | Holds |
|---|---|---|
| System store | the `--storage` location | dashboards, charts, datasets, saved statements, chat history, favourites, permissions, audit |
| Engine catalog | `/var/lib/kaveon/catalog.db` on the coordinator (the `catalog-data` volume) | catalogs, schemas, every table definition, planner statistics, cubes |

With `file://` the system store is a directory — copy it. With `s3://` or
`adls://` use the provider's own versioning and retention. The catalog database
is a file on a Docker volume; copy it while the coordinator is stopped, or
`sqlite3 catalog.db ".backup"` while it is running — a plain copy of a live
SQLite database with a write-ahead log can be torn.

**Verify a restore.** A backup nobody has restored from is not yet a backup, and
this is not a hypothetical here: on 2026-09-14 a restore brought the records
back and left the Engine catalog empty, because the catalog database was on a
coordinator volume and in no snapshot. Twenty-eight table registrations were
rebuilt by hand.

## Replacing the host

Losing the machine is not the same as losing the data. Table data in object
storage is untouched, and the system store with it if you chose `s3://` or
`adls://`. What does not come back on its own is the Engine catalog, so a new
host comes up able to read everything and knowing about nothing.

With the catalog database restored from backup, the stack comes up as it was.
Without it, rebuild the map:

```bash
# 1. Bring the stack up against the same system store, so the dashboards,
#    charts and datasets are already there.
./scripts/kaveon-up.sh --storage adls://myaccount/system/kaveon

# 2. Re-register each catalog, its schemas and its tables. Columns are read
#    from the tables' own metadata; re-running skips what already exists.
python scripts/register-lake-catalog.py --api http://localhost:8082 \
    --catalog OpenSource \
    --account myaccount --container opensource --root snapshots/2026-09-09-v1 \
    --schema public:covid_global,nyc_taxi_borough \
    --schema ai_benchmarks:arena_battles,leaderboard,pricing

# 3. Re-measure. Until a table is analyzed, every question against it is a
#    full read, so the dashboards are correct but slow.
#    ANALYZE per table, from SQL Lab or the Catalog page.
```

Two things to expect on step 3. Statistics and cubes are not quick to rebuild:
the cube pass gives each worker a single task over all of its files with a hard
600-second ceiling, so a large table may need more workers to fit inside it —
see [the lake](../engineering/lake-delta-and-cube.md). And a dashboard tile that
was answered from a cube goes back to scanning until its table's cube exists
again, which shows up as a tile that is right and slow rather than one that is
broken.

## Upgrading

```bash
git pull
./scripts/kaveon-up.sh --build
```

The system store carries its own format version and the Engine refuses to start
against a store it does not understand, rather than writing something older
Kaveon cannot read.
