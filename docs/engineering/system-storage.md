# System storage: one configured store is the system of record

**Status:** design, 2026-10-07. Supersedes nothing; describes a change that is
partly already true and names the part that is not.

## The principle

One object store, chosen once at deployment, holds **every piece of metadata
Kaveon owns**: catalog definitions, schemas, table definitions, statistics,
cubes, product records, audit. Nothing Kaveon knows about itself lives anywhere
else, and nothing it knows about itself lives only on a node.

Data is the opposite. A table can live in any store the deployment can reach —
a second ADLS account, an S3 bucket, a local directory, a Delta or Iceberg
location someone else writes. Those are **data sources**. The *record* of them —
that the catalog exists, what its schemas are, where each table points, what has
been measured about it — is written to the system store, always.

So: **where a table's bytes live is a per-table decision. Where Kaveon's memory
of that table lives is a deployment-wide one, made once.**

## What is already true

The product transaction store is already this, for product records. It writes
immutable snapshots and advances a single head with a conditional write, so a
commit is atomic and a reader either sees the whole of it or none of it. It
carries an operation index, so a retried commit is recognised rather than
duplicated. Since `8184001d` it rebases and retries a commit that lost the race
rather than refusing it.

It is also already storage-agnostic in the place that matters.
`AdlsConditionalCommit` wraps an `Arc<dyn ObjectStore>`; the only ADLS-specific
code is the workload-identity constructor. `object_store` is compiled with both
the `azure` and `aws` features. A local filesystem mode already exists for
development. **S3 is a constructor, not a port.**

And `CatalogSnapshot` already carries `tables` and `table_statistics` beside the
product records.

## The gap

The Engine's own catalog — catalogs, schemas, table definitions, statistics,
cubes, the catalog audit log — lives in **SQLite on the coordinator**, at
`config.catalog_database_path`. On the personal Azure deployment the coordinator
container app has `volumes: null` and `mounts: null`, so that file sits on
ephemeral container storage. Every revision replacement starts with an empty
catalog.

This is not hypothetical. On 2026-09-14 a restore brought PostgreSQL back and
the Engine catalog was empty, because it was SQLite on a coordinator volume and
not in any snapshot; 28 table registrations were rebuilt by hand.

The result today: **the data is durable and the map to it is not.**

It also explains a defect fixed in `24be3dd8` — `system.catalogs`, `.schemas`
and `.tables` are a Parquet projection of that SQLite catalog, materialized by a
script that had to reach back into the Engine over HTTP to rebuild them.

## The design

### One store, named once

A single configuration choice names the system store:

    KAVEON_SYSTEM_STORAGE = adls://<account>/<container>/<prefix>
                          | s3://<bucket>/<prefix>
                          | file:///<path>

It is read at startup and never changes for the life of a deployment. Changing
it is a migration, not a setting, and the Engine refuses to start if the store
it is pointed at holds a head written for a different identity.

Credentials follow the scheme: workload or managed identity for `adls://`, the
standard provider chain for `s3://`, nothing for `file://`. No credential enters
a catalog definition; a definition carries a reference to one, as it does today.

This replaces `KAVEON_PRODUCT_STORAGE_MODE`, `KAVEON_PRODUCT_ADLS_ACCOUNT`,
`KAVEON_PRODUCT_ADLS_CONTAINER`, `KAVEON_PRODUCT_ADLS_PREFIX` and
`KAVEON_PRODUCT_LOCAL_PATH` with one value that says the same thing once. The
old variables keep working for one release and are read as a fallback.

### The catalog becomes part of the snapshot

Catalogs and schemas join tables and statistics in `CatalogSnapshot`, as
first-class entries with their own change variants carrying their own
preconditions — an expected revision on update and delete, absence on create —
exactly as `CreateProduct`/`UpdateProduct`/`DeleteProduct` do. That matters
beyond tidiness: write sets whose every change states its own precondition are
the ones the commit path is allowed to rebase, so catalog DDL gets the same
concurrency behaviour product writes just gained.

A catalog registration, a schema creation, a table definition, an `ANALYZE`
result and a cube all become changes in one commit protocol against one store.

### SQLite stays, as a derived read cache

Not as an authority. The read path — `table_by_name`, the joins behind
discovery and the Catalog page — is worth keeping fast and relational, and
rebuilding it from the head is cheap at this size.

- **Startup**: read the head, materialize SQLite from it, serve reads.
- **Mutation**: commit to the system store; on `Committed`, apply to SQLite.
- **Divergence**: SQLite records the snapshot reference it was built from. If it
  does not match the head, it is discarded and rebuilt. It is a cache, so
  throwing it away is always safe.
- **Loss**: an empty or missing SQLite file is not an error. It is rebuilt.

That single property is what closes the gap: a coordinator with no disk at all
comes up with the full catalog, because the catalog was never on its disk.

### Other catalogs, other storage

A catalog definition keeps its `StorageType` — local path, ADLS account and
container, S3 bucket, and whatever is added later. A deployment whose system
store is ADLS can register a catalog over an S3 bucket, and vice versa. The
definition of that catalog, its schemas, its tables and everything measured
about them are written to the system store like everything else.

The system store is therefore the one place to back up, the one place to
restore, and the one place that answers "what did Kaveon know".

## Migration

1. Add `SystemStorage` — the URL form, the three constructors, and the identity
   check — with the existing product variables as a fallback. No behaviour
   change.
2. Add catalog and schema entries to `CatalogSnapshot` with precondition-bearing
   change variants, behind a flag, written alongside SQLite. Both are updated;
   SQLite is still read.
3. Build SQLite from the head at startup, and compare it against the SQLite that
   bootstrap would have produced. Qualify that they agree on a real deployment.
4. Flip the read path to the materialized cache and make the commit the only
   writer. SQLite becomes derived.
5. Delete the bootstrap-from-environment path. A catalog exists because someone
   registered it, not because a directory was present at startup.

Each step is separately shippable and separately reversible, and no step
requires the deployment to be down.

## What this is not

It is not a distributed catalog service, and it does not make the coordinator
stateless in general — query state, spill files and the result cache stay local
and are meant to. It is about one thing: Kaveon's memory of itself belongs in
the store the deployment chose, not on whichever machine happened to serve the
request that created it.
