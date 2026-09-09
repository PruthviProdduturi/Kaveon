# ADLS transaction protocol

This is the durable state path for Kaveon product metadata and, eventually,
Engine catalog/schema/table definitions. It uses ADLS Gen2 objects only.
SQLite, PVCs, and coordinator-local files are not durable state. Temporary
memory or upload buffers may be lost before a successful head compare-and-swap.
It is a **target protocol**: Kaveon does not yet implement or qualify this
writer, reader, recovery path, or transaction guarantee against an ADLS account.

The protocol relies on Azure Blob ETags: a write with `If-Match` succeeds only
when the supplied ETag is current; a mismatch returns HTTP 412. Microsoft
documents this in [Manage concurrency in Blob Storage](https://learn.microsoft.com/azure/storage/blobs/concurrency-manage).
That documentation also specifies strong consistency for subsequent reads and
lists after an insert or update. The protocol nevertheless treats listings as
non-authoritative: only a directly read head and its digest-verified manifest
establish committed state.
Blob versioning is not available for hierarchical-namespace ADLS Gen2 accounts,
as documented in [Blob versioning overview](https://learn.microsoft.com/azure/storage/blobs/versioning-overview),
so this protocol does not depend on it for head recovery. One conditional head
write is the only visible commit point. Azure's documented storage primitives
are not, by themselves, evidence of a Kaveon transaction implementation.

## Object layout

All paths are tenant-scoped and use opaque identifiers. Names contain no emails,
SQL, secret references, or customer text.

```text
product-catalog/
  objects/<sha256>/data.parquet                 immutable table/index data
  manifests/<commit-id>.json                    immutable complete snapshot
  intents/<tenant>/<idempotency-key-hash>.json  immutable request digest/commit id
  head-history/<commit-id>.json                 immutable read-back head record
  head-backups/<commit-id>.json                 immutable recovery evidence
  heads/catalog-head.json                       sole mutable commit pointer
  gc/marks/<run-id>.json                        immutable GC evidence
```

`objects` and `manifests` are created with `If-None-Match: *`, checksums, and a
canonical encoding. A manifest contains its commit and parent IDs, monotonically
increasing catalog revision, actor/role, request and trace IDs, idempotency-key
hash/request digest, validation summary, and **every** logical table’s schema,
row/object layout, immutable key indexes, count and digest. It references
unchanged objects from the parent as well as replacements.

The head holds only current manifest path/digest, revision, commit ID and a
bounded idempotency lookup segment. It contains no product records or secrets.
A reader gets the head once, verifies its manifest digest, then uses that
manifest for the whole request. It never rereads current head mid-request.

## Commit algorithm

1. Authenticate and derive tenant, immutable subject, role, request ID and
   canonical mutation digest. Reject reuse of an idempotency key with a different
   digest.
2. Read the head and ETag; validate and pin its complete manifest.
3. Resolve the idempotency key through the head’s retained immutable lookup
   index. If present, return its prior commit result without a write.
4. Apply the typed request in memory to the pinned snapshot. Validate role,
   schema, lifecycle, primary/unique keys, foreign references and all bounds.
   Build immutable replacement data/index objects and one complete manifest.
5. Upload objects and manifest. A retry verifies an existing checksum, never
   overwrites. Write immutable intent with `If-None-Match: *`; it is not commit
   authority.
6. Replace `heads/catalog-head.json` with the new pointer using `If-Match` and
   the ETag from step 2. Success is the commit.
7. Read back the exact head and verify its commit ID, manifest digest and
   revision. Write immutable `head-history` and `head-backups` records containing
   those exact bytes, ETag, digest and parent commit. They are recovery evidence,
   not a second commit authority.
8. On HTTP 412, candidate objects remain unreachable. Read current head and
   revalidate from that snapshot; never reuse earlier constraint decisions.

The head serializes commits for a tenant/catalog. Independent heads/shards are a
future protocol change requiring a separate atomicity proof; one transaction may
not update multiple heads.

## Ambiguous responses and idempotency

If head CAS succeeds but the response is lost, the client retries with the same
tenant-bound idempotency key and canonical digest. The service reads the head’s
retained idempotency index and returns the original commit ID/revision/result.
If the key is reserved by an intent but absent from current head, the service
follows retained manifest parents/checkpoint indexes without blob listing. It
reports uncommitted only after reaching the retention boundary and verifying the
intent digest.

Entries have a minimum retention longer than client retry and source-outbox
replay windows. Compaction preserves them in immutable lookup segments referenced
by head. After the documented window, reuse is rejected as expired rather than
silently replayed. No local cache decides commit outcome.

## Validation and bounds

Initial table formats use sorted immutable primary-key segments and immutable
unique/FK index segments. The manifest maps table/key ranges to segments, so a
proposed key or reference has bounded lookup cost rather than a Parquet scan.

Set protocol limits for tables, rows, bytes, object count, key lookups, index
shard rewrites, manifest size, and CAS retries. Exceeding one fails before CAS;
it is never split into visible partial commits. Large imports prepare immutable
batches but publish only through a bulk checkpoint with the same single-head
rule.

Schemas are versioned manifest entries. Writers accept only product-model
compatible changes. Constraints cover catalog-source names, user recents,
dataset children, chart/dataset references, visibility values, lifecycle states
and owner/tenant scope. Product code uses repository operations; arbitrary SQL
is not part of this protocol.

## Head recovery and garbage collection

Objects uploaded before failed CAS are harmless orphans. Blob listings are
advisory only for GC and never choose a committed snapshot, recover idempotency,
or establish transaction order.

After every successful read-back, the service writes an immutable app-level
history/backup record. It retains a checkpointed parent chain and exact head
payload, ETag, manifest digest and verification time. These records make a
recovery candidate auditable but do not make a pre-CAS candidate committed.

If the mutable head is unavailable or corrupt, the service fails closed: it
fences reads and writes. Recovery restores only an operator-selected, exact
head payload from a verified app-level backup after checking its manifest digest,
parent chain and referenced immutable objects. The restored head is written with
an empty-head precondition when it is absent, or the observed corrupt ETag when
it remains present, and is followed by a new recovery audit record. The procedure
must produce evidence of the selected backup, digest checks, restore result and
any declared recovery-point loss. It must never select the highest revision or
newest manifest from a blob listing; those files can be failed-CAS orphans. ADLS
backup/restore and this procedure require real-account failure qualification
before the catalog is authoritative.

GC roots are the directly read head, retained app-level head-history/checkpoint
records, rollback pins, active migration roots and minimum idempotency/read
windows. GC traverses manifest references, writes immutable mark evidence, waits
longer than maximum reader/retry duration, rechecks roots, then deletes only aged
unmarked objects. Storage retention and soft delete can be defense in depth but
are neither transaction authority nor a replacement for application history.

## Migration, cutover, telemetry and qualification

PostgreSQL remains authoritative during transition. For each family, write a
source outbox row in the same PostgreSQL transaction as the source mutation,
take a repeatable snapshot/watermark, import through this protocol, replay the
outbox idempotently and reconcile counts, IDs, ownership, visibility, references,
revisions and payload hashes. Cutover needs an API write fence, drained final
watermark, successful reconciliation and auditable read switch. Rollback fences
ADLS writes and returns reads to retained PostgreSQL; a snapshot is not a reverse
write log.

Emit non-sensitive metrics for commit/revision, CAS attempts/conflicts,
validation category, objects/bytes, latency, idempotency replay/expiry, GC,
migration watermark/lag, mismatches, fence duration and rollback state. Audit
keeps immutable actor subject and request/trace IDs, never credentials, raw SQL,
chat, cache payloads or customer values.

Qualify against the real ADLS account and workload identity: cross-table success
and rollback, CAS races, every retry failure point, lost-response idempotency,
coordinator restart, checksum failure, explicit head corruption/deletion and
verified backup restore, GC versus pinned reader, bounds/schema/authorization,
source backfill/outbox replay, final fencing and cutover rollback.

## Implemented foundation and remaining gates

The repository now contains these internal components, without a public write
endpoint or a product storage cutover:

- `storage::AdlsConditionalCommit`: create-only writes, matching-body/ETag reads,
  bounded streaming reads, and conditional replacement. Storage tests: 39 passed.
- `catalog::product_manifest`: versioned complete table-reference snapshots and
  bounded multi-table preparation. It does not validate row-level constraints.
- `catalog::product_commit::ProductCatalogCommit`: immutable snapshot JSON,
  a digest-verified head, conditional publication, and bounded history lookup.
  Catalog tests: 18 passed, including transaction metrics. Both crates pass
  strict Clippy. The live ADLS REST primitive probe also passed; see
  [its evidence](adls-commit-validation-2026-09-09.json).
- `catalog::product_metrics`: attempts, in-flight operations, commit/replay/
  conflict/rejection/indeterminate/abandonment outcomes and bounded latency
  buckets. These are internal process counters, not yet an exposed monitoring
  endpoint or durable audit stream.

The publication prototype assumes trusted callers validate referenced objects,
row/schema/foreign-key constraints, authorization, and a canonical request
fingerprint before calling it. It does not write table data. Head reads are
limited to 64 KiB and snapshot reads/writes to 8 MiB. New heads reference immutable, digest-verified operation-index shards, so
idempotency checks no longer depend on a 64-hop history walk. Each shard is
bounded to 1,024 entries in this prototype; capacity exhaustion refuses writes
until shard splitting is implemented. Legacy heads without an index require
explicit migration and fail closed. No automatic history deletion is implemented.

Still required: the Delta/Parquet mutation writer and constraint indexes,
authenticated transactional API, scalable index splitting, independently
verified head recovery, failure-injection coverage for lost write responses,
telemetry exposure/audit/alerts, every application repository adapter, and the
backfill/reconciliation/fencing/cutover/rollback suite. Existing PostgreSQL and
SQLite Engine definition storage remain live. No full transaction, HA, or
migration readiness claim follows from these foundation tests.
