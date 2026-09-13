# Storage and catalogs

Arrow `RecordBatch` is the execution unit. Storage exposes synchronous streaming readers with strict projection validation. Unknown, empty, or duplicate columns fail. Storage predicates conservatively eliminate Parquet row groups while execution filters preserve row-level correctness.

Local Delta replays contiguous JSON logs from version zero and reads active Parquet files. Checkpoints, deletion vectors, broader schema evolution, and incomplete-history recovery remain pending. Parquet row groups and Delta files form deterministic scan partitions. Telemetry measures file/row-group selection, compressed bytes, output, footer/read/snapshot time, and throughput.

The native SQLite/WAL catalog provides transactions, migrations, stable IDs, optimistic revisions, lifecycle validation, structured Arrow schemas, credential references, and audit history. Secrets are forbidden. Workers execute coordinator-resolved fragment sources rather than consulting mutable local catalogs.

The platform PostgreSQL source registry and Engine catalog are separate stores,
connected by the native catalog synchronization API. Saving a platform source
does not import data or register its tables. See the
[registration guide](../guides/register-engine-catalog.md) for the current
workflow and validation limits. Canonical `abfss://` Parquet locations execute
through Azure object-store range reads with environment/workload identity or
Azure CLI authentication. See the Engine qualification documentation for the
supported readers and remaining format/adapter limitations.

## Cloud read boundary

An ADLS Parquet scan pins the object identity returned by `HEAD` (ETag or
version, together with size) before loading the footer. Footer, object metadata,
and decoded batches are cached only under that identity. A range read uses the
same identity with conditional `GET`; replacement of an object therefore fails
closed and invalidates the metadata cache instead of returning mixed data.

Parquet range requests are bounded to 16 in-flight requests per reader. The
bound preserves request order while preventing a wide projection or many row
groups from turning one scan into an unbounded connection and memory burst.
Large immutable objects may use the bounded full-object cache; its process-wide
byte limit is explicit and eviction is safe because active readers retain their
lease. Exact source statistics are likewise keyed by the pinned object identity
or Delta snapshot version. The cache is an optimization only: if identity or
statistics cannot be established, the reader returns an error rather than
guessing.
