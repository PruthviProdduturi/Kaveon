# Kaveon — Engineer Coordination

> **Read this file at the start of every session.** This is how Claude and Codex stay in sync without a middleman.

## How this works

- Both engineers read this file before writing code
- When you ship something, update the status table below
- When you define or change an interface, update the contracts section
- The repo is the communication channel — no human relay needed
- If a contract changes, the engineer who changes it updates this file in the same commit

---

## Ownership map

### Engine (`engine/`)

| Crate | Owner | Status |
|-------|-------|--------|
| `core` — shared types, errors, traits | Shared (either can add, neither restructures without updating this doc) | Stage graph, exchange, split/task, plan, telemetry, catalog, and operator contracts active |
| `catalog` — durable Engine metadata | **Codex** | Native SQLite/WAL catalog, revisions, lifecycle, audit, and coordinator API complete; multi-coordinator service and external adapters remain target |
| `storage` — Parquet reader, ADLS Gen 2 | **Codex** | Local Parquet row-group and Delta active-file splits are enumerable and partitioned; ADLS Gen 2 not started |
| `exec/scan` — scan operator | **Codex** | Done; takeover authorized 2026-09-03 |
| `exec/aggregate` — hash aggregate | **Codex** | Partial/final state execution and exchange for COUNT/SUM/MIN/MAX/AVG/exact DISTINCT done |
| `exec/filter` — filter evaluation | **Codex** | Compatible numeric coercion done |
| `exec/sort` — sort operator | **Codex** | Local/distributed planning done; fixed-fan-in external merge available |
| `exec/topn` — TopN operator | **Codex** | Local/distributed planning done; fixed-fan-in external merge available |
| `sql` — parser, logical plan | **Codex** | Equi/cross joins and exact COUNT DISTINCT done |
| `optim` — filter pushdown | **Codex** | Done and wired in local/server planning |
| `python` — PyO3 bindings | **Claude** | Scaffold |
| `cli` — `kaveon` interactive SQL shell | **Codex** | Remote-first client done; embedded mode requires `--local` |
| `benches` — Criterion benchmarks | **Codex** | Reproducible storage and execution Criterion suites done; external PostgreSQL/Trino harness pending |

### API (`api/`)

| Area | Owner | Status |
|------|-------|--------|
| DLM engine (`api/dlm/`) | **Claude** | Done (shipping) |
| DLM standalone API extraction | **Claude** | Not started |
| Routers, services, middleware | **Claude** | Done (shipping) |
| Catalog source CRUD (`api/routers/catalog_sources.py`) | **Claude** | In progress — Entra-authorized endpoints, Key Vault credential refs, audit events |
| Adapter configuration (Hive Metastore, AWS Glue, Unity Catalog, Iceberg REST) | **Claude** | In progress — optional adapter config on catalog sources |

### Studio (`studio/`)

| Area | Owner | Status |
|------|-------|--------|
| Platform rebrand (about, landing, nav) | **Claude** | About page and shared wordmark redesigned; broader landing polish pending |
| Lakehouse data source UI / Catalog Sources admin | **Claude** | In progress — storage type, credential ref, adapter config, lifecycle management |
| UX polish | **Claude** | In progress |

### Launch

| Area | Owner | Status |
|------|-------|--------|
| README rewrite | **Claude** | Done; current/alpha/target boundaries documented |
| Docker Compose | **Claude** | Engine compose present; deployment validation ongoing |
| Demo datasets | **Claude** | Not started |
| White papers | **Claude** | Present; evidence and maturity audit in progress |
| Architecture diagrams | **Claude** | Unified platform set rebuilt |

---

## Interface contracts

### Catalog hierarchy (Trino-style)

```rust
// Defined in kaveon-core::catalog

// Catalog → Schema → Table, configured per catalog
pub enum StorageType {
    Local { base_path: PathBuf },
    AdlsGen2 { account, container, root_path },
    S3 { bucket, region, prefix },
}

pub enum AccessPattern { Shortcut, Optimized }
pub enum DataFormat { Parquet, Delta, Iceberg }

pub struct TableMeta {
    pub name: String,
    pub arrow_schema: SchemaRef,
    pub location: String,          // relative path within storage
    pub access: AccessPattern,
    pub format: DataFormat,
}

// TableReference::parse("catalog.schema.table") resolves through CatalogManager
pub struct CatalogManager { catalogs, default_catalog, default_schema }
```

- **Claude** owns the catalog types, `MemoryCatalog`, and `CatalogManager`
- **Codex** uses `ResolvedTable.full_path()` + `StorageType` to open the right storage backend
- SQL planner resolves table names via `CatalogManager.resolve_table()` before building physical plan

### Storage → Exec boundary

Storage produces batches. Exec consumes them.

```rust
// Defined in kaveon-core::operator

pub trait BatchSource {
    fn schema(&self) -> &SchemaRef;
    fn next_batch(&mut self) -> Result<Option<RecordBatch>>;
}

pub trait BatchOperator {
    fn schema(&self) -> &SchemaRef;
    fn next_batch(&mut self) -> Result<Option<RecordBatch>>;
}
```

- **Codex** implements `BatchSource` on the Parquet reader
- **Claude** consumes `BatchSource` in the scan operator, implements `BatchOperator` on scan/filter/aggregate/sort

Local Parquet API (`kaveon-storage`):

```rust
let source = ParquetReader::new(path)
    .with_batch_size(8_192)
    .with_columns(vec!["region".into(), "revenue".into()])
    .with_predicate(predicate)
    .read()?; // ParquetBatchIterator: Iterator<Item = Result<RecordBatch>> + BatchSource

let metadata = ParquetReader::new(path).metadata()?;
```

- Projection is strict: empty, duplicate, and unknown columns return `KaveonError::Storage`
- Row-group pruning uses `kaveon_core::StoragePredicate` and is conservative when statistics are absent or inexact
- Predicates eliminate row groups only; execution operators still apply row-level filters
- `ScanPartition::new(index, count)` assigns each Parquet row group or Delta active file to exactly one partition by stable ordinal; invalid partition coordinates return `KaveonError::Storage`

### Predicate type (Storage-level)

```rust
// Defined in kaveon-core::predicate

pub enum StoragePredicate {
    Compare { column: String, op: CompareOp, value: ScalarValue },
    IsNull { column: String },
    IsNotNull { column: String },
    In { column: String, values: Vec<ScalarValue> },
    And(Vec<StoragePredicate>),
    Or(Vec<StoragePredicate>),
    Not(Box<StoragePredicate>),
}
```

- **Codex** uses this for row-group pruning in storage
- **Claude** maps logical plan filter expressions to `StoragePredicate` when building the physical plan
- **Codex** uses this for filter pushdown in optim

### SQL → Exec boundary

```rust
// Defined in kaveon-sql::logical_plan

pub enum LogicalPlan {
    Scan { table, alias, columns },
    Join { left, right, join_type, condition },
    Filter { input, predicate },
    Project { input, columns },
    Aggregate { input, group_by, aggregates },
    Sort { input, order_by },
    Limit { input, count },
}
```

- `AggregateExpr::Count { expr, distinct }` preserves exact DISTINCT semantics.
- Join types are inner, left, right, full, and cross; the physical hash join currently accepts equality-key conjunctions, while cross join has no condition.
- Relation aliases are retained so joined output columns remain qualified and ambiguous unqualified references fail explicitly.
- **Codex** owns the SQL parser, logical plan, optimizer, and CLI/server physical-plan translation.

### Engine → API boundary (PyO3 target)

```python
import kaveon_engine

result = kaveon_engine.execute("SELECT ...", "/path/to/data")
version = kaveon_engine.version()
```

- **Claude** owns PyO3 bindings
- Current binding is a scaffold and is not integrated into the shipping API
- Target contract is `execute(sql, data_path) -> dict` and `version() -> str`

### Engine operational API

- `GET /v1/query` returns up to 100 most-recent in-memory query records for the Engine UI
- Records include SQL, live/terminal state, schema, rows, error, submission/completion time, measured analysis/planning/execution/serialization phases, logical plan, and completed storage-scan metrics
- Storage-scan records include file and row-group selection, pruning, selected compressed bytes, emitted rows/batches, Delta snapshot time, Parquet footer time, read time, and throughput
- Statement clients may provide optional `source`, `client`, `time_zone`, `client_tags`, and `result_delivery` context; the coordinator records its actual version, environment, default catalog, and default schema. Principal and client address remain unavailable until authenticated request plumbing exists.
- Logical plans are returned as structured `PlanNode` trees. Optimized and physical plan fields remain nullable until their producers are wired.
- Completed distributed queries expose measured stage/task telemetry: worker, partition, elapsed time, output rows, Arrow batches, and transport bytes. Physical operator CPU/memory, blocked time, spill, and live task updates remain unavailable.
- History is process-local and resets when the coordinator restarts
- Node payloads and heartbeats include `memory_rss_bytes`, measured from each Engine process; unsupported hosts report zero
- Workers advertise a routable `KAVEON_ADVERTISED_URI` and execute internal `POST /v1/task` partition requests. Task control is JSON; typed results use Arrow IPC stream transport. With at least two active workers, the coordinator distributes single-source COUNT, SUM, MIN, MAX, GROUP BY, and scan-backed ORDER BY + LIMIT, then performs the appropriate final merge. AVG, DISTINCT, and joins currently fall back to node-local execution.

### Remote CLI target

```text
kaveon --server http://coordinator:8080 --catalog kaveon --schema default
kaveon --server http://coordinator:8080 --execute "SELECT COUNT(*) FROM customers"
```

- The installed `kaveon` binary must be a thin coordinator client by default, comparable to the Trino CLI. It must not open customer storage or execute operators in the client process.
- Interactive metadata commands use the coordinator catalog endpoints; SQL uses `POST /v1/statement`, and returned query IDs link directly to Engine query history/details.
- `--server`, `--catalog`, `--schema`, `--user`, `--source`, `--client-tags`, `--execute`, output format, timeout, TLS, and future Entra token options belong to the client contract.
- Existing embedded execution may remain only behind an explicit `--local` mode during migration.
- Installable Windows, Linux, and macOS binaries come from Engine release artifacts; package-manager installers are follow-up distribution work.

### Explain and execution telemetry

```rust
// Defined in kaveon-core::telemetry
pub struct PlanNode { id, phase, operator, attributes, children }
pub struct PlanMetricsSnapshot { sequence, captured_at_unix_ms, nodes }
pub struct NodeMetrics { operator: OperatorMetrics, scan: Option<ScanMetrics> }
```

- Plan node IDs are stable within a query and join live or terminal metric snapshots to the physical plan.
- Every measurement is optional so consumers can distinguish an unsupported metric from a measured zero.
- Durations use nanoseconds, sizes use bytes, timestamps use Unix milliseconds, and counters are monotonic.
- Storage attaches `ScanMetrics` to scan nodes; execution operators attach `OperatorMetrics` without depending on server or UI types.
- Planner, operator, coordinator, and UI wiring remain separate follow-up work owned by their respective components.

### Distributed exchange foundation

```rust
// Defined in kaveon-core::exchange
pub struct StageId(pub u32);
pub struct TaskId { query_id, stage_id, partition, attempt }
pub enum Partitioning { Single, Hash { columns, partition_count }, Broadcast, RoundRobin { partition_count } }
pub struct StageGraph { query_id, root_stage, stages, exchanges }
pub struct StageFragment { id, task_count, plan }
pub struct ExchangeDescriptor { id, source_stage, target_stage, partitioning }
pub struct TaskAssignment { task_id, node_id, splits }
```

- `StageGraph::validate` rejects duplicate/unknown/cyclic stages and exchanges and invalid partitioning before scheduling.
- `HashPartitioner` uses Arrow row encoding plus fixed FNV-1a hashing so equal keys, including nulls, always reach the same partition on every worker running the same Engine version.
- `BoundedExchangeBuffer` and the HTTP `ExchangeStore` enforce byte ceilings; the store also bounds exchange count and releases exact payload accounting on cleanup. Streaming wait/wake flow control remains future work.
- The v2 server exchange envelope carries query/exchange/stage/task-attempt/output-partition identity, versioning, chunk bounds, and corruption checks around Arrow IPC payloads. Including `ExchangeId` prevents collisions when a task feeds multiple downstream stages.
- General fragment tasks fetch producer exchanges, aggregate inputs by exchange ID, execute their assigned scan partition, and publish hash/single/round-robin/broadcast outputs to consumer workers.
- Internal exchange routes use authenticated `POST`, `GET`, and `DELETE /v1/internal/exchange/...`; every node must share a non-empty `KAVEON_EXCHANGE_TOKEN`. Upload retries are idempotent, conflicting duplicates fail closed, failed attempts and consumed stages clean up their outputs.

### Memory reservations

```rust
let query = QueryMemoryPool::new(query_id, query_limit_bytes)?;
let operator = query.operator("hash-aggregate")?;
let reservation = operator.reserve(bytes)?;
```

- Reservations atomically enforce the query-wide hard limit across operator accounts and release through RAII.
- Snapshots expose current and peak bytes for the query and operator. Operators and admission control are not wired to these accounts yet; operator-triggered spill is not implemented.
- `SpillManager` writes bounded Arrow IPC runs into collision-safe private directories, accounts current/peak disk bytes, rolls back failed writes, streams replacement runs, and removes runs through RAII.
- Sort and TopN expose opt-in spill-aware constructors with a validated merge fan-in (16 by default). Multi-pass compaction and lazy final merge bound memory to the fan-in cursor batches plus one output batch; aggregate/join spill and admission control remain open.

### Distributed stage planning

- `build_stage_graph(query_id, logical_plan, worker_count)` produces and validates post-order stage DAGs.
- Grouped aggregates use hash exchange; global aggregates, sort, and final TopN use single-partition exchange.
- Equi-joins hash both inputs into colocated partitions; cross joins model a round-robin probe side and broadcast build side.
- `build_executable_fragments` translates the same stage IDs and exchange IDs into validated worker plans, including partial/final aggregates and repartitioned/broadcast joins.
- `CoordinatorOrchestrator` assigns workers deterministically, gates dependent stages, rotates retry attempts, carries exact scan partitions, and resolves producer/consumer exchange locations and cleanup.
- The coordinator runs eligible scan/filter/project/aggregate/sort/limit/join trees through authenticated fragment tasks before legacy fallbacks, collects only root Arrow results, and fails closed after partial distributed execution.
- `ExecutableFragment` is the versioned worker instruction contract for scans, filter/project, aggregate modes, Sort/TopN/limit, exchange inputs/outputs, and hash joins. It rejects malformed, cyclic, unreachable, and invalid operator graphs before execution.
- `WorkerLifecycle` now backs HTTP task owner/waiter/completed replay and cancellation. Coordinator cancellation propagates to workers and retains `CANCELED` history; an authenticated terminal-finish endpoint releases registry capacity without conflating completion and cancellation.

### Sort and TopN execution

```rust
let ordering = vec![
    SortExpr::new(Expr::Column("revenue".into()), false).with_nulls_first(false),
];
let sorted = SortOperator::new(input, ordering.clone())?;
let top_ten = TopNOperator::new(input, ordering, 10)?;
```

- `SortExpr::new(expr, ascending)` defaults to SQL-style null ordering: nulls last for ascending and first for descending; `with_nulls_first` applies an explicit SQL `NULLS FIRST/LAST` choice.
- `SortOperator` performs lexicographic ordering across all input batches and emits bounded output batches (8,192 rows by default).
- `TopNOperator` uses Arrow's limited lexicographic index selection and returns at most the requested number of rows; a zero limit does not consume its input.
- Claude's CLI/server physical planners map `LogicalPlan::Sort { order_by }` to `SortOperator`, constructing each `SortExpr` from `(Expr, ascending)`. When a `Limit` directly wraps a `Sort`, they may fuse it to `TopNOperator`; otherwise use `LimitOperator` over `SortOperator`.

### Filter pushdown optimization

```rust
let optimized = kaveon_optim::rules::push_filter_down(logical_plan);
let storage_predicate = kaveon_optim::rules::to_storage_predicate(&filter_expr);
```

- `push_filter_down` moves filters through sort and direct-column projections, rewriting simple aliases, while preserving the row-level filter at the scan boundary.
- Limit, aggregate, computed/ambiguous projection, and unsupported expression boundaries are not crossed.
- `to_storage_predicate` accepts column/literal comparisons, `IS NULL`, `IS NOT NULL`, safe conjuncts from `AND`, and fully convertible `OR`/`NOT` trees. It rejects arithmetic, functions, column-to-column comparisons, `NULL` comparisons, and partially convertible disjunctions.
- Claude's physical planners apply the optimizer before translation and attach a converted predicate from `Filter(Scan)` to the storage reader while retaining the execution filter.

### Local Delta snapshot API

```rust
let source = DeltaTableReader::new(table_directory)
    .with_columns(vec!["region".into(), "revenue".into()])
    .read()?;
```

- The local reader replays ordered `_delta_log/*.json` add/remove actions and streams only active Parquet files.
- Snapshot reads require a complete contiguous JSON history beginning at version 0; checkpoint replay is not implemented and incomplete histories fail closed.
- CLI and server planners select Parquet or Delta readers from `TableMeta::format`.
- `CatalogManager::set_default(catalog, schema)` validates and changes CLI defaults without unloading catalogs.

### Native catalog

- `kaveon-catalog` is the single-coordinator durable metadata authority, backed by SQLite transactions, WAL, foreign keys, schema migrations, optimistic revisions, lifecycle validation, and audit history.
- Catalog definitions store only `CredentialReference` values; secrets, tokens, passwords, and connection strings are forbidden from metadata and audit records.
- `ColumnDefinition` persists Arrow `DataType` structurally, including nested and parameterized types. Display strings are not a serialization contract.
- The coordinator reconstructs the process-local `CatalogManager` planning snapshot from durable active definitions at startup and after mutations.
- Catalog mutations are coordinator-only and require `Authorization: Bearer <KAVEON_CATALOG_ADMIN_TOKEN>` plus `x-kaveon-actor`. An absent admin token disables mutation endpoints.
- `ExecutableFragment` version 2 carries the coordinator-resolved source URI and `DataFormat`. Workers execute that immutable resolution and do not consult their local catalog snapshot.
- SQLite is not a multi-coordinator claim. The scale target is an external transactional catalog service with PostgreSQL persistence and revision-aware invalidation.
- Hive Metastore, AWS Glue, Unity Catalog, and Iceberg REST are capability contracts only; adapters are not implemented. See `engine/CATALOG.md`.

### DLM API (standalone target)

- **Claude** extracts DLM into callable API endpoints independent of Studio
- Standalone extraction and its OpenAPI contract are not implemented yet
- Auth: same proxy secret model as main API

---

## Rules

1. **Never cross ownership boundaries** without updating this doc first
2. **Shared types go in `core`** — both engineers can add to core, but update contracts section here
3. **If you change a trait signature**, update this doc in the same commit
4. **If you're blocked on the other engineer's work**, note it in the status table ("Blocked: waiting on X")
5. **Test against the contract, not the implementation** — mock the other side if needed
6. **CI is the referee** — if it passes, you haven't broken anything

---

## Log

| Date | Engineer | What changed |
|------|----------|-------------|
| 2026-09-01 | Claude | Created HANDSHAKE.md, defined shared types in core (BatchSource, BatchOperator, StoragePredicate) |
| 2026-09-01 | Codex | Storage reader in progress against BatchSource/StoragePredicate contracts; fixed CatalogList::catalog_mut trait-object lifetime blocking workspace compilation |
| 2026-09-01 | Claude | Added Expr/BinaryOp to core. Built production hash aggregate (GroupKey hashing, SUM/COUNT/AVG/MIN/MAX, null handling). Rewrote scan to consume BatchSource trait. Built filter operator with expression evaluator. Implemented SQL→LogicalPlan translator (SELECT/WHERE/GROUP BY/ORDER BY/LIMIT). Removed sql→exec circular dep. |
| 2026-09-01 | Claude | Added Trino-style catalog system to core: Catalog→Schema→Table hierarchy, StorageType (Local/ADLS Gen2/S3), AccessPattern (Shortcut/Optimized), DataFormat (Parquet/Delta/Iceberg), CatalogManager with table reference resolution, MemoryCatalog implementation. |
| 2026-09-01 | Codex | Completed local Parquet M1: streaming BatchSource, strict projection, metadata, typed StoragePredicate row-group pruning, 8 passing tests, strict Clippy. Full workspace check blocked by sqlparser 0.53 API mismatches in kaveon-sql (ValueWithSpan, GroupByExpr, OrderBy, Value). |
| 2026-09-01 | Codex | Engine CI run 33578907744: storage is formatted, tested, and Clippy-clean; workspace format gate is blocked on Claude-owned cli/{config,display,main,planner}, core/{catalog,operator}, exec/{aggregate,expr_eval,project}, and sql/logical_plan. Codex formatted owned benches/aggregate.rs. |
| 2026-09-01 | Codex | Storage→exec integration compile reaches kaveon-exec, then blocks on Claude-owned expr_eval imports removed from arrow::compute in Arrow 54 (eq/neq/lt/lt_eq/gt/gt_eq); aggregate.rs also has unused num_rows. Storage itself compiles cleanly. |
| 2026-09-01 | Codex | Added reproducible Criterion storage benchmarks for full scans, projection, and row-group pruning. The benchmark target compiles and storage passes strict Clippy; workspace formatting remains blocked by formatting drift in Claude-owned crates. Cross-engine runs will use identical Parquet/Delta data and resource limits, with local mounts for correctness and shared object storage for representative lakehouse performance. |
| 2026-09-01 | Codex | Architect-requested documentation pass: rewrote ARCHITECTURE.md to separate shipping, alpha, and target behavior; added theme-aligned accessible SVGs for the three-pillar platform, Engine pipeline, and deployment topology. No runtime contract changed. |
| 2026-09-02 | Codex | Rebuilt product/architecture branding and About experience, removed obsolete local-password authentication paths, restored strict Studio type checking, and aligned public documentation with current/alpha/target behavior. |
| 2026-09-02 | Codex | Redesigned the Engine operational UI and added server-backed `GET /v1/query` history. Removed fabricated timing phases; the UI reports only measured Engine telemetry. |
| 2026-09-02 | Codex | Added a root Vercel upload boundary so the Studio deployment excludes Engine build artifacts, backend sources, local caches, and secrets. |
| 2026-09-02 | Codex | Restored the cinematic About presentation and integrated platform maturity as a native three-pillar block; aligned the canonical wordmark, responsive navigation, reduced-motion behavior, and accessible dashboard controls. |
| 2026-09-02 | Codex | Audited root documentation against commit `464911f`; corrected deployment, connector, authentication, and maturity claims and added code-grounded documentation indexes for API and configuration. |
| 2026-09-02 | Codex | Expanded Studio docs into a product-wide portal with Engine, API, Operations, and Research sections; centralized navigation, published the architecture diagram set, and aligned current/alpha/target claims with runtime behavior. No public runtime contract changed. |
| 2026-09-02 | Codex | Hardened the documentation portal with full-content search, maturity and verification metadata, accessible navigation, code copying, SQL and connector capability matrices, troubleshooting, upgrade and release guidance, and blocking CI documentation checks. No Engine runtime contract changed. |
| 2026-09-02 | Codex | Unified the About and documentation navigation under one crisp public Kaveon header and removed the duplicate docs wordmark treatment. No runtime contract changed. |
| 2026-09-02 | Codex | Rebuilt root Docker Compose as a complete localhost stack: Studio, API/DLM, PostgreSQL metadata and data databases, and the Engine coordinator/workers. Studio now builds reproducibly inside its container; explicit local mode enables the development identity only for this loopback-bound stack. |
| 2026-09-02 | Codex | Hardened the localhost stack after clean-runner startup testing: Studio explicitly binds every container interface and health probes its public docs route over IPv4; the local API uses one worker to serialize its legacy runtime schema bootstrap. |
| 2026-09-02 | Codex | Made the Studio container package registry configurable through `KAVEON_NPM_REGISTRY` and persisted its BuildKit package store across retries, preserving npmjs as the portable default while supporting managed Docker proxy environments. |
| 2026-09-02 | Codex | Applied the same configurable, retry-safe package-feed contract to the API image through `KAVEON_PIP_INDEX_URL`; the portable default remains PyPI. |
| 2026-09-02 | Codex | Matured the Engine operational console into a read-only control-plane experience with stronger information hierarchy, responsive query search/state filters, catalog inventory, explicit refresh, and a clearer separation from Studio. Engine API contracts are unchanged. |
| 2026-09-02 | Codex | Added measured per-process RSS memory to Engine node heartbeats and moved coordinator uptime into the header. The console now charts active workers, observed query count, and aggregate Engine RSS at five-second intervals for the current browser session. |
| 2026-09-02 | Codex | Restored the Engine summary to a balanced five-card row and added current aggregate Engine RSS alongside its session trend. Catalog inventory remains the distinct registry view for configured data sources. |
| 2026-09-02 | Codex | Bounded Engine node rendering to 12 cards per page with node-name search, role filtering, and pagination so large clusters remain operationally useful. |
| 2026-09-02 | Codex | Removed the root localhost stack's duplicate `local` catalog registration; `/data` now appears once under the canonical `kaveon` catalog while explicit catalog configuration remains supported. |
| 2026-09-02 | Codex | Removed the duplicate Active Workers trend from the Engine console; current worker availability remains in the summary while the trend area is reserved for non-duplicative query and memory telemetry. |
| 2026-09-02 | Codex | Simplified the Engine overview to aggregate operational signals: removed per-worker details, total-node duplication, and catalog inventory from the UI. Active worker count remains visible; catalogs remain internal Engine query-routing state. |
| 2026-09-02 | Codex | Added correct local multi-file Delta snapshot reads via JSON transaction-log replay, immediate Delta table discovery for CLI/server catalogs, format-aware physical scans, and validated CLI `USE`; verified six tables and 50,100,500 rows from `F:\kaveon-data`. |
| 2026-09-02 | Codex | Rebuilt Engine query history as an operator-focused execution list with state, query identity, submission time, SQL, elapsed time, returned rows, columns, and drill-down while avoiding unsupported Trino metrics. |
| 2026-09-02 | Codex | Added the shared structured plan and execution-metric contract for explain, live plan snapshots, and storage scan telemetry; no planner or operator implementation changed. |
| 2026-09-02 | Codex | Wired real query lifecycle phases and completed Parquet/Delta scan telemetry into Engine query records and the Plan view; physical operator and distributed-stage instrumentation remain the next milestone. |
| 2026-09-02 | Codex | Rebuilt Query Details from the Trino information model with explicit session/execution/Engine context and a responsive structured logical-plan tree; unsupported identity and physical telemetry remain visibly unavailable. |
| 2026-09-02 | Codex | REQUEST @Claude: convert the owned `kaveon` CLI from embedded execution to the documented thin `--server` coordinator client; keep embedded execution only as explicit `--local`. The current coordinator statement/catalog APIs are the alpha transport. |
| 2026-09-02 | Codex | REQUEST @Claude: unify the owned About/PublicHeader composition. The fixed near-black header currently separates from the cinematic hero; retain navigation contrast but sample the hero atmosphere through translucent color, shared glow/grid geometry, and a seamless first-section transition. Validate desktop/mobile and reduced motion. |
| 2026-09-02 | Codex | ARCHITECT DECISION @Claude: restore the exact pre-`1111483` About-specific header treatment and navigation (`Kaveon`→About, Features anchor, Docs, GitHub, Launch App; 12px glass blur and subtle divider) while preserving the current About body. Do not apply this rollback to the docs header. |
| 2026-09-02 | Codex | Implemented the architect-directed, scoped restoration of the pre-`1111483` About header through an About-only `PublicHeader` variant; Docs navigation and the current About body remain unchanged. |
| 2026-09-03 | Codex | Corrected the About-only restored header after rendered verification: removed the legacy translucent grey glass surface, matched the hero canvas, and preserved the branded blue launch action. Docs remains unchanged. |
| 2026-09-03 | Codex | Fixed Studio's empty/unavailable dataset initialization path so the Ask input exits “Loading your data context” instead of waiting forever when no dataset schemas can be loaded. |
| 2026-09-03 | Codex | Rebalanced the About-only public header after 1710px rendered review: enlarged the wordmark, established a 72px rail, replaced tiny theme-overridden links with deliberate navigation controls, and strengthened the primary action. Docs remains unchanged. |
| 2026-09-03 | Codex | Removed the global-theme background leaking behind the About wordmark, tightened its left inset, removed the header/hero divider, and replaced implementation-disclaimer hero copy with the unified platform value proposition. |
| 2026-09-03 | Codex | Audited the CLI against the remote-client contract: the current binary remains an embedded local shell and lacks `--server`, remote `--execute`, transport/auth/output options, and release packaging. Corrected the ownership status; Claude's existing remote-client request remains open. |
| 2026-09-03 | Codex | Completed vectorized multi-batch Sort and TopN operators with lexicographic ASC/DESC ordering, explicit null placement, bounded sort output batches, Arrow limited TopN selection, strict configuration/expression errors, and 13 passing unit tests. REQUEST @Claude: wire `LogicalPlan::Sort` in the CLI/server physical planners using `SortExpr`; fuse directly wrapped `Limit(Sort)` to `TopNOperator`. |
| 2026-09-03 | Codex | Replaced the aggregate placeholder with deterministic, throughput-labeled hash-aggregate and vector filter/arithmetic benchmarks across multiple group cardinalities; documented a correctness-gated PostgreSQL/Trino comparison protocol. External cross-engine execution remains pending. |
| 2026-09-03 | Codex | Completed conservative filter pushdown and Expr-to-StoragePredicate conversion with 10 focused tests; residual filters remain for row-level correctness. REQUEST @Claude: wire optimizer output into CLI/server physical planning. BLOCKED correctness evidence: real Delta Float64 columns compared with Int64 SQL literals fail in Claude-owned expression evaluation because numeric coercion is missing; planner/evaluator must coerce compatible numeric operands before execution. |
| 2026-09-03 | Codex | ENGINE READINESS BLOCKERS @Claude: `COUNT(DISTINCT customer_id)` is silently planned as ordinary `COUNT` (returned 5,000,000 on local orders), joins are rejected, CLI/server planners currently discard `ORDER BY`, and the CLI is still embedded-only. These are release-gate failures: implement or explicitly reject unsupported DISTINCT syntax, add join logical/physical execution, wire Sort/TopN and optimizer passes, and complete the remote coordinator CLI before Engine can be labeled ready. |
| 2026-09-03 | Codex | ARCHITECT AUTHORIZATION: Codex takes ownership of the remaining Engine SQL, aggregate/filter correctness, physical planner wiring, and remote CLI readiness work previously assigned to Claude. Engine paths may be changed as required; non-Engine ownership is unchanged. |
| 2026-09-03 | Codex | Closed the functional Engine readiness blockers: remote-first CLI with request-scoped catalog/schema/user context, optimizer and storage pruning integration, physical Sort/TopN, exact COUNT DISTINCT, compatible numeric coercion, and INNER/LEFT/RIGHT/FULL/CROSS hash joins with qualified relation aliases. Real Delta verification returned 100,000 distinct customers, 4,039,740 orders above 100, descending IDs 4,999,999..4,999,997, a 5,000,000-row customer/order join, and correct regional joined counts. Remaining scale limitation: joins materialize locally without spill or distributed exchange; this is not yet a Trino-class distributed performance claim. |
| 2026-09-03 | Codex | Added projection pruning for scans and relation-aware join pruning. Optimized local release measurements on `F:\kaveon-data`: exact distinct over 100K rows 83 ms, filtered count over 5M rows 527 ms, TopN over 5M rows 247 ms, and 5M-row joined regional aggregate 6.09 s (improved from 15.93 s before join pruning). Results are single runs, not comparative benchmark claims. |
| 2026-09-03 | Codex | Added the first correctness-bounded distributed execution slice: deterministic Parquet row-group and Delta-file partitions, routable worker advertisements, internal worker task execution, concurrent coordinator fan-out, and partial COUNT/SUM/MIN/MAX GROUP BY merge. AVG, DISTINCT, joins, ordering, limits, exchange, retry, and spill intentionally remain local/target. |
| 2026-09-03 | Codex | Replaced worker-result JSON with Arrow IPC streams and added completed-stage/task telemetry to query records and the Engine UI, including worker, partition, elapsed time, rows, batches, and transport bytes. This establishes the binary exchange contract; live updates and operator CPU/memory/spill metrics remain follow-up work. |
| 2026-09-03 | Codex | Added shared stage/task/attempt and partitioning contracts, deterministic multi-column Arrow hash partitioning, and a byte-bounded exchange queue with explicit backpressure. Correctness tests cover deterministic assignment, equal/null keys, lossless row coverage, invalid configuration, and capacity release; network shuffle is not wired yet. |
| 2026-09-03 | Codex | Began the eight-workstream distributed-runtime program: added validated stage/fragment/exchange/split contracts, mergeable weighted AVG and exact COUNT DISTINCT states, a bounded authenticated Arrow exchange wire envelope, and alternate-worker partition retry. Added `engine/DISTRIBUTED_EXECUTION_STATUS.md` as the durable cross-machine handoff. Network endpoints, general fragment scheduling, distributed TopN/join, spill, cancellation propagation, and mature split scheduling remain explicit gates. |
| 2026-09-03 | Codex | Added distributed partial/final TopN, authenticated bounded exchange endpoints, atomic query/operator memory reservations, and dynamic split leasing with failed-task requeue. Local Docker nodes now share the exchange token through environment configuration. General fragment execution, distributed join/AVG/distinct, operator memory wiring/spill, cancellation, and scheduler stress remain open. |
| 2026-09-04 | Codex | Added versioned Arrow encoding for weighted AVG and typed exact DISTINCT states, a validated aggregate/sort/TopN/join stage-DAG builder, bounded RAII Arrow spill runs, and task timeout/retry classification. These are green foundations; stage execution, distributed join/state exchange, operator spill wiring, cancellation, and Docker/AKS stress remain open. |
| 2026-09-04 | Codex | Added the dependency-gated stage runtime, bounded idempotent task/cancellation lifecycle, and opt-in spill-aware Sort/TopN. Final spill merge remains eager, and runtime/lifecycle contracts still require worker HTTP integration before failure recovery or fully bounded execution can be claimed. |
| 2026-09-04 | Codex | Added versioned executable fragments, canonical grouped aggregate-state transport, lazy spill-run merging, idempotent HTTP task replay/cancellation, and authenticated terminal lifecycle cleanup. Fragment translation/execution and distributed join/AVG/distinct remain the next correctness gates; spill merge fan-in remains uncapped. |
| 2026-09-04 | Codex | Wired the general distributed runtime: graph-to-fragment translation, authenticated coordinator dispatch, partition-correct worker execution, collision-free exchange v2 transport, partial/final AVG and exact DISTINCT state execution, repartitioned/broadcast joins, retry/cancel/cleanup, fixed-fan-in spill merge, and deterministic local Parquet/Delta split enumeration. Workspace tests and strict Clippy pass; Docker fault/performance evidence, aggregate/join spill, admission control, ADLS Gen2, and AKS remain explicit gates. |
| 2026-09-04 | Codex | Two-worker Docker validation on the real local Delta data passed exact DISTINCT (100,000), grouped weighted AVG plus TopN (three stages), TopN, and the 5,000,000-row customer/order repartition join. The run found and fixed Rust 1.88 image compatibility, finalized-aggregate projection mapping, HTTP exchange envelope limits, and canonical grouped-state hash partitioning. Full Kaveon Compose was restored healthy after Engine validation. |
| 2026-09-04 | Claude | Claimed control-plane integration: catalog source CRUD API (Entra-authorized, Key Vault credential refs, audit events), Studio Data Sources admin UI (storage type/format/adapter/lifecycle management), optional adapter config for Hive Metastore/AWS Glue/Unity Catalog/Iceberg REST. Schema aligned with Engine's CatalogDefinition/CredentialReference/CatalogAdapter types. Codex retains Engine-native catalog, transactional persistence, and runtime resolution. |
| 2026-09-04 | Codex | Added the Engine-native durable catalog: validated shared definitions and Arrow schemas, SQLite/WAL transactions and migrations, stable IDs/revisions, lifecycle enforcement, credential references, audit history, authenticated coordinator CRUD, restart reconstruction, and persistent Docker coordinator storage. Executable fragment v2 carries resolved format/location so workers cannot diverge through stale local catalogs. External catalog adapters and multi-coordinator metadata remain explicit targets. |
| 2026-09-04 | Codex | REQUEST @Claude: bridge the Entra-authorized platform `catalog_sources` lifecycle to the authenticated Engine catalog definition APIs with stable ID mapping, revision conflict propagation, credential references only, and actor attribution. The PostgreSQL source registry and Engine catalog are separate authorities until this bridge is implemented and tested; Studio registration must not imply Engine query availability yet. |
