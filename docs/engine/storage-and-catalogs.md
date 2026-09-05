# Storage and catalogs

Arrow `RecordBatch` is the execution unit. Storage exposes synchronous streaming readers with strict projection validation. Unknown, empty, or duplicate columns fail. Storage predicates conservatively eliminate Parquet row groups while execution filters preserve row-level correctness.

Local Delta replays contiguous JSON logs from version zero and reads active Parquet files. Checkpoints, deletion vectors, broader schema evolution, and incomplete-history recovery remain pending. Parquet row groups and Delta files form deterministic scan partitions. Telemetry measures file/row-group selection, compressed bytes, output, footer/read/snapshot time, and throughput.

The native SQLite/WAL catalog provides transactions, migrations, stable IDs, optimistic revisions, lifecycle validation, structured Arrow schemas, credential references, and audit history. Secrets are forbidden. Workers execute coordinator-resolved fragment sources rather than consulting mutable local catalogs.

The platform PostgreSQL source registry and Engine catalog remain separate until the catalog bridge ships. Canonical `abfss://` Parquet locations execute through Azure object-store range reads with environment/workload identity or Azure CLI authentication. Cloud Delta-log replay, S3, richer Delta, Iceberg, and external catalog adapters remain pending execution paths.
