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

Re-running the script is safe. Secrets already in `.env` are preserved.

## The one decision: where Kaveon keeps its own memory

`--storage` names the single store that holds **everything Kaveon knows about
itself** — catalog definitions, schemas, table definitions, statistics, cubes,
dashboards, charts, datasets, permissions, audit.

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
`ANALYZE` on each. A table whose statistics are current answers counts, totals
and column bounds *without reading the data*; one that has never been analyzed is
read in full for every question. The Catalog page says which is which.

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
./scripts/kaveon-up.sh             # start again, same secrets
```

Back up the system store. That one location holds every definition and every
record; the data it points at is backed up wherever it lives. With `file://` it
is a directory — copy it. With `s3://` or `adls://` use the provider's own
versioning and retention, and **verify a restore**, because a backup nobody has
restored from is not yet a backup.

## Upgrading

```bash
git pull
./scripts/kaveon-up.sh --build
```

The system store carries its own format version and the Engine refuses to start
against a store it does not understand, rather than writing something older
Kaveon cannot read.
