# ADLS transaction protocol

This is the durable state path for Kaveon product metadata and, eventually,
Engine catalog/schema/table definitions. It uses ADLS Gen2 objects only.
SQLite, PVCs, and coordinator-local files are not durable state. Temporary
memory or upload buffers may be lost before a successful head compare-and-swap.

The protocol relies on Azure Blob ETags: a write with `If-Match` succeeds only
when the supplied ETag is current; a mismatch returns HTTP 412. Microsoft
documents this in [Manage concurrency in Blob Storage](https://learn.microsoft.com/azure/storage/blobs/concurrency-manage).
That documentation also specifies strong consistency for subsequent reads and
lists after an insert or update. The protocol nevertheless treats listings as
non-authoritative: only a directly read head and its digest-verified manifest
establish committed state.
Blob versioning should be enabled for head recovery; Azure documents that each
write produces a blob version and versions are immutable in its [versioning
overview](https://learn.microsoft.com/azure/storage/blobs/versioning-overview).
Those features do not create a multi-blob transaction. One conditional head
write is the only visible commit point.

## Object layout

All paths are tenant-scoped and use opaque identifiers. Names contain no emails,
SQL, secret references, or customer text.

```text
product-catalog/
  objects/<sha256>/data.parquet                 immutable table/index data
  manifests/<commit-id>.json                    immutable complete snapshot
  intents/<tenant>/<idempotency-key-hash>.json  immutable request digest/commit id
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
7. On HTTP 412, candidate objects remain unreachable. Read current head and
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

## Recovery and garbage collection

Objects uploaded before failed CAS are harmless orphans. Blob listings are
advisory only for GC and never choose a committed snapshot, recover idempotency,
or establish transaction order.

GC roots are current head, retained prior head versions, rollback pins, active
migration/checkpoint roots and minimum idempotency/read windows. GC traverses
manifest references, writes immutable mark evidence, waits longer than maximum
reader/retry duration, rechecks roots, then deletes only aged unmarked objects.
Soft delete/version retention are recovery guards, not transaction authority.

For retained data, use Azure WORM only on immutable paths. Microsoft documents
that WORM retention prevents overwrites/deletes in [Immutable Storage for Blob
Data](https://learn.microsoft.com/azure/storage/blobs/immutable-storage-overview).
Do not apply a policy that blocks the mutable head’s next CAS; separate it from
WORM-protected objects or use a compatible version-level policy. Lifecycle
deletion must include version handling as documented in [lifecycle management
policy structure](https://learn.microsoft.com/azure/storage/blobs/lifecycle-management-policy-structure).

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
coordinator restart, checksum failure, GC versus pinned reader, bounds/schema/
authorization, source backfill/outbox replay, final fencing and cutover rollback.
