# Where KaveonDB stands: a rated comparison with Trino and PostgreSQL

> Assessed September 12, 2026 against `dev` at `432a05d`, Engine digest `4a165e94…` on `kaveon-test-aks`, Trino 483 and PostgreSQL 18 documentation. Every number in this paper is either a measurement recorded in `HANDSHAKE.md` with a query ID, or a documented behaviour of the compared system. Nothing here is a marketing claim; the last section says what may be claimed.

## How to read the scores

KaveonDB is compared on two axes because it is trying to be two things. Against **Trino** it is a distributed analytical engine over lake files; against **PostgreSQL** it is a transactional store for the product's own records. Each dimension carries a score out of 10 for KaveonDB **relative to the reference system doing that job in production today**, with the evidence that earned it and the gap that capped it. A 10 means "no reason to choose the reference over KaveonDB on this dimension"; a 1 means "the capability is absent." The scores are not averaged into one number, because an average would hide the two dimensions that decide whether the product can ship.

Two facts shape everything below:

- The Engine is ~10 weeks old. Trino traces to Presto (2012); PostgreSQL to POSTGRES (1986).
- KaveonDB is not sold as a general database. It is the query and record layer of one product, and it is allowed to be narrow where the product is narrow.

---

## Part 1 — Analytics: KaveonDB versus Trino

### 1.1 Throughput on a matched workload — **7 / 10**

**Evidence.** The only comparison that counts is the resource-matched, exact-result, concurrency-four run of the twelve-query corpus (count, selective filter, arithmetic projection, low/medium/high-cardinality aggregation, multi-aggregate, exact distinct, TopN, equi-join, grouped join) on the same Parquet files with SHA-256-verified inputs. On AKS at the accepted digest lineage, two independent 120-query probes completed 120/120 at **2.836592 and 2.832527 QPS (mean 2.834559)**. Trino 483 on the same files and limits recorded **2.244578 QPS**. That is 26.3 % above Trino. The declared objective is 1.90× (4.265 QPS); today's result is 66.5 % of it.

**What the number does not say.** The corpus is 10 M-row tables; warm cache; four concurrent clients; one coordinator and three 3-CPU workers. Cold-cache, mixed-workload soak, and larger data are separate experiments that have not been run. The first single-node six-query diagnostic in early September was 1.057×; the gain since is real engineering (shared worker clients, amortised reservations, worker-local exact DISTINCT, dense integer grouping, parallel Delta metadata), each accepted or rejected on measured evidence.

**Why not higher.** 1.26× on one warm corpus is a promising engine, not a faster engine. Trino's optimizer, dynamic filtering and join reordering have not been exercised by this corpus. The score would move to 8 with a passed cold-cache run and to 9 with the 1.90× gate.

### 1.2 Scale: the 504 M-row test — **5 / 10**

**Evidence.** On September 11 the 504,000,000-row `kaveon_events_enriched` (one 6.43 GB Parquet file, 168 row groups) was registered on KaveonDB and queried through the Studio front door:

| Statement | Result | Time |
|---|---|---|
| `COUNT(*)` | 504,000,000 exact from footer statistics | 0.3 s |
| `SELECT surface, SUM(actions) … GROUP BY surface` (2 columns) | 6 rows, exact | 20.5–22.2 s |
| Same, 9 aggregates | 6 rows, exact | 60–205 s |
| `GROUP BY country`, year predicate | 26 rows | 32.7 s |
| `COUNT(DISTINCT user_id)` (3 M distinct) | workers OOM-killed at 6 GiB (old digest); fix landed in `3531c68`, not yet qualified | — |
| `MIN(event_date)` on Utf8 | rejected "requires a numeric column"; fix landed in `3531c68` | — |

On the previous digest (`6f33810b…`) a worker retained memory across statements and died on the *second* wide scan; digest `4a165e94…` held at 268 MiB peak through a full DLM build (886.8 s, zero restarts). That is the correct direction, found by a real workload within a day.

**Trino** on the same three 3-CPU workers would scan the file at a comparable rate — the arithmetic is bandwidth and decode — but with mature spill for aggregation and join, dynamic filtering, and no OOM on exact distinct of 3 M keys.

**Why 5.** The Engine reads 504 M rows correctly and fast enough for a build, but ~20 s for a two-column aggregate is not interactive, and the wide-aggregate cost (3–10× the narrow one) says projection and decode are not yet tight. Trino would answer both in the same order of magnitude; the difference is that Trino would not have needed a fix to survive it.

### 1.3 SQL surface — **6 / 10**

**Supported today** (`engine/crates/sql`): SELECT with WHERE/GROUP BY/HAVING/ORDER BY/LIMIT, non-recursive CTEs, CASE, CAST to the supported types, arithmetic and string projection, IN/NOT IN/EXISTS/NOT EXISTS subqueries in WHERE, UNION/INTERSECT/EXCEPT (distinct), window functions ROW_NUMBER/RANK/DENSE_RANK/LAG/LEAD/SUM/AVG/COUNT/MIN/MAX OVER with ROWS/RANGE/GROUPS frames, EXTRACT, DATE_TRUNC/DATE_PART/TO_CHAR/NOW/CURRENT_DATE/CURRENT_TIMESTAMP, Decimal128, COUNT/SUM/MIN/MAX/AVG and exact DISTINCT.

**Explicitly rejected, with a message** rather than a wrong answer: recursive CTEs, correlated subqueries, INTERSECT ALL/EXCEPT ALL, named windows, window NULL treatment / FILTER / WITHIN GROUP, DISTINCT in window functions, NULLS FIRST in window ordering, BEGIN with isolation or access-mode modifiers, COMMIT AND CHAIN, SAVEPOINT. Fail-closed on unsupported syntax is a deliberate and correct choice for a young engine; the differential suite (37 cases against Trino 483 and DuckDB) proves it.

**Trino** additionally has: lateral and correlated subqueries, GROUPING SETS/CUBE/ROLLUP, array/map/row/JSON types and functions, approximate aggregates (`approx_distinct`, `approx_percentile`), geospatial, regular-expression and hundreds of scalar functions, table functions, prepared statements, EXPLAIN ANALYZE.

**Why 6.** For the dashboard and DLM-generated shapes the product emits, the surface is complete and honest. For a human analyst in SQL Lab it is noticeably narrower than Trino, and the first missing thing they hit is usually a correlated subquery, a `GROUPING SETS`, or an `approx_distinct`.

### 1.4 Storage and formats — **5 / 10**

**KaveonDB** reads Parquet (row-group pruning by statistics, projection pushdown), Delta (transaction-log replay pinned to one version; no checkpoints), and Iceberg, from local disk and ADLS Gen2 via workload identity. `StorageType::S3` exists as a type with no implementation. Writes are limited to the immutable product-record protocol (below). Statistics are exact and immutable: Parquet footers and Delta active snapshots, cached per planning pass — no file-size heuristics.

**Trino** reads and writes Hive, Iceberg, Delta and Hudi tables with schema evolution, partition evolution, time travel, `CREATE TABLE AS`, `INSERT`, `MERGE`, `DELETE`, compaction, and does so on S3, GCS, ADLS, HDFS and MinIO. Its Delta connector handles checkpoints and deletion vectors.

**Why 5.** Three readers on one cloud, no writes, no S3, no Delta checkpoints. The read side is precise where it exists (exact statistics are better than Trino's estimates for broadcast decisions), but the surface is a fraction of Trino's.

### 1.5 Federation and connectors — **1 / 10**

Trino's reason to exist is querying many systems as one: 40+ connectors, cross-catalog joins, pushdown into JDBC sources, Kafka, Elasticsearch, MongoDB, and system tables. KaveonDB has no connector concept: it reads lake files. Kaveon *the product* federates through the API's connection pool (Fabric SQL, Azure SQL, PostgreSQL, MySQL, StarRocks) — but that is the API executing SQL on those systems, not the Engine joining across them. This is a category KaveonDB has chosen not to enter; it is scored so the reader knows that.

### 1.6 Distributed execution and fault tolerance — **6 / 10**

**KaveonDB**: coordinator plus workers over authenticated HTTP with Arrow IPC exchange; deterministic Parquet row-group and Delta file splits; partial/final aggregates, distributed Sort/TopN, repartitioned and broadcast hash joins; task attempts with alternate-worker retry, cancellation propagation, and exchange cleanup; verified on AKS with forced in-flight worker deletion (attempt-1 retry to an alternate worker, exact result preserved), coordinator restart reconciliation of abandoned exchange directories, and 12/12 concurrent exact queries under bounded pressure.

**Trino**: the same architecture matured over a decade, plus fault-tolerant execution (Tardigrade) with spooled exchanges that survive worker loss for long queries, graceful shutdown, spill for every blocking operator, adaptive planning, and dynamic filtering.

**Why 6.** The primitives are there and tested against failure, which is more than most young engines. What is missing is what only time and production give: spill for aggregate and join (Sort/TopN have it), streaming exchange flow control, and a sustained mixed-workload soak.

### 1.7 Memory and admission — **5 / 10**

**KaveonDB**: hard reservations, a per-query limit (512 MB default), a per-process admission limit (2 GiB), a hash-spill budget (4 GiB), byte-bounded exchange storage, resource groups with an explicit wildcard admission group for unenumerated principals, and per-task telemetry (partition, copy, IPC, spill, queue delay). The 504 M-row build showed the limit was not yet covering everything on `6f33810b…` (a worker exceeded 6 GiB while the query limit said 512 MB) and that `4a165e94…` closes most of it.

**Trino**: the same concepts, plus `query.max-memory-per-node`, memory revocation with spill on every operator, and years of tuning under multi-tenant load.

**Why 5.** Bounded by design, proven leaky by a real table, fixed within a day — but "fixed on the second try under my first big table" is not the same as "holds under a tenant's worst query."

### 1.8 Optimizer — **4 / 10**

KaveonDB has filter pushdown, projection pruning, join pruning, exact-statistics broadcast eligibility (fail-closed), and dense-integer grouping. It has no cost-based join reordering, no dynamic filtering, and no adaptive re-planning. Trino has all three. On single-table aggregates — the product's dominant shape — the gap barely shows; on the grouped-join queries in the corpus it is where the remaining 1.90× lives.

### 1.9 Clients and ecosystem — **2 / 10**

KaveonDB speaks its own HTTP statement API and a remote-first `kaveon` CLI. There is no JDBC/ODBC driver, no PostgreSQL or Trino wire compatibility, no Python DB-API package, and therefore no direct access from Tableau, Power BI, dbt, Superset, DBeaver or notebooks. Trino has all of these and a REST protocol other tools already implement. The product compensates because Kaveon Studio *is* the client, but any customer who wants their existing tools on the data is blocked at this line.

### 1.10 Security and multi-tenancy — **6 / 10**

KaveonDB: TLS, Entra-validated bearer identity with fail-closed issuer/tenant/subject checks, a bridge token between API and Engine with per-request principal and role headers, owner-bound query and transaction records, catalog revision conflicts (`If-Match`), and encrypted credentials with an explicit keyring and migration. Trino adds row filters and column masks, Ranger/OPA integration, query-level impersonation, and audit events. KaveonDB's model is tighter for a single product; Trino's is broader for an enterprise.

### 1.11 Operability — **5 / 10**

KaveonDB ships reproducible images, Helm and Bicep, health/readiness/metrics, a query history with per-stage telemetry, the KaveonDB console in Studio, immutable digest rollouts, and startup reconciliation. What it does not have: backup/restore of the catalog, an upgrade/rollback qualification, autoscaling evidence, and the operational runbooks Trino accumulated (graceful decommission, resource group tuning guides, JMX metrics). The console is arguably nicer than Trino's; the depth beneath it is thinner.

### Analytics summary

| Dimension | KaveonDB vs Trino | Decides shipping? |
|---|:---:|:---:|
| Throughput, matched corpus | 7 | yes — 1.90× gate |
| Scale at 504 M rows | 5 | yes — interactive latency |
| SQL surface | 6 | no |
| Storage and formats | 5 | partly — S3 |
| Federation | 1 | no (out of scope) |
| Distributed execution, fault tolerance | 6 | yes — soak |
| Memory and admission | 5 | yes |
| Optimizer | 4 | no |
| Clients and ecosystem | 2 | partly — depends on positioning |
| Security | 6 | no |
| Operability | 5 | partly — backup/restore |

**Standing.** On the workload it was built for, KaveonDB already completes more exact queries per second than Trino on matched resources, and it does so with exact statistics and fail-closed semantics that Trino does not have. Everywhere outside that workload — federation, SQL breadth, ecosystem, optimizer — Trino is years ahead, and no amount of engine work in the next ten weeks changes that. The honest claim is "faster than Trino on our workload, narrower everywhere else," and the product is designed so that narrowness rarely shows.

---

## Part 2 — Transactions: KaveonDB versus PostgreSQL

### 2.1 What KaveonDB's transactional layer is

It is a **typed product-record protocol on object storage**, not a relational database. Records are datasets, charts, dashboards, DLM definitions, favourites, saved queries, themes, sources — the product's own metadata. Each write is a revision-checked, digest-verified immutable document; publication is a conditional (compare-and-swap) head update on ADLS Gen2 using HNS versioning and snapshots; reads are snapshot-bound with explicit snapshot-isolation metadata; conflicts and indeterminate outcomes are surfaced as proofs, not swallowed. `BEGIN`/`COMMIT`/`ROLLBACK` are accepted for exactly one statement per request; isolation modifiers and SAVEPOINT are rejected by name. There are no user tables, no row DML, no constraints, no indexes beyond uniqueness checks on record keys.

Migration from PostgreSQL is running under a fail-closed protocol: datasets (9/9), charts (70/70), dashboards (8/8) and DLM definitions (9/9) have been reconciled exactly into KaveonDB on AKS from a retained PostgreSQL snapshot, with an outbox for ongoing writes, checkpointed backfills, and a receipt verifier. Query history's first live write is blocked by an Engine validation response. **PostgreSQL remains authoritative**; no cutover, fencing, or destructive cleanup has been authorised.

### 2.2 Scores

| Dimension | KaveonDB vs PostgreSQL | Evidence and gap |
|---|:---:|---|
| ACID for the product's records | **6** | Atomic publication by CAS head update, durable on ADLS with versioning, snapshot isolation metadata on reads, conflict proofs; 53 catalog tests. No multi-record transaction across kinds, no WAL-equivalent recovery qualification, no crash-consistency soak. |
| General relational DML | **1** | INSERT/UPDATE/DELETE exist only for `product.*` record kinds and set `document_json`. No user tables, no arbitrary rows. This is by design; the score records the fact. |
| Constraints and indexes | **2** | Uniqueness on record keys and reference validation (a chart's dataset must exist; corrected on September 11 so an owner may own many datasets). No secondary indexes, no foreign keys as a general facility, no CHECK. |
| Isolation and concurrency | **4** | Snapshot reads, revision conflicts, indeterminate-outcome proofs. No row locking, no serialisable, no long transactions; one statement per request. PostgreSQL's MVCC has forty years on this. |
| Durability and recovery | **4** | Object-storage durability is excellent; the protocol's own recovery (verified head recovery, index shard splitting, restart of an interrupted publication) is partly qualified. No point-in-time recovery, no backup/restore of the record store. |
| Query capability over records | **5** | Records are readable through the Engine's SQL with bounded reads and typed views; the DLM definition kind is queryable. No joins across record kinds with ad-hoc predicates the way `SELECT … FROM charts JOIN datasets` works today in PostgreSQL. |
| Latency for OLTP-shaped operations | **3** | A publication is an ADLS conditional PUT: tens of milliseconds at best, hundreds under contention; PostgreSQL commits in sub-millisecond on local disk. Acceptable for "save a chart," not for anything chatty. |
| Ecosystem | **1** | No wire protocol, no drivers, no psql, no extensions, no ORMs. |
| Operational maturity | **3** | Retained snapshots and a fail-closed migration are good discipline; PostgreSQL has replication, logical decoding, pg_upgrade, and every DBA on earth. |

**Standing.** KaveonDB is not a PostgreSQL replacement and the repository says so in every relevant document. It is on track to be the *product's* record store — a narrower job — with a stronger consistency story on object storage than most lakehouse metadata layers and a migration protocol that refuses to lie about its own progress. The dimensions where it scores 1–2 are dimensions the product does not need; the dimensions where it scores 3–4 (recovery, isolation, latency) are the ones that must reach 6+ before PostgreSQL can be retired.

---

## Part 3 — Where the product is strong on both axes

These are the things neither Trino nor PostgreSQL offers, and they are why the comparison is not simply "immature engine":

1. **One store, two workloads, no ETL.** The same Parquet/Delta files serve analytics and the record store lives beside them on the same account. Trino needs a metastore and an external database for anything transactional; PostgreSQL needs an extractor to feed a lake.
2. **Exact statistics, fail-closed.** Broadcast decisions and `COUNT(*)` come from immutable footers and pinned snapshots. Trino estimates; a wrong estimate costs a spill, a wrong count costs a dashboard.
3. **Deterministic natural language over both.** The DLM compiles a dataset's context (74 dimension values and nine totals for the 504 M-row table; 3,474 curated cuboids for the showcase dataset) and answers the same question the same way every time, from context in ~0.13 s or live on the Engine with the SQL shown. Neither reference system has a semantic layer.
4. **A console that belongs to the product.** Cluster, queries, stages and memory in Studio with the same identity and roles as everything else.

---

## Part 4 — What may be claimed today

- "KaveonDB completes 26 % more exact queries per second than Trino 483 on the declared twelve-query corpus at matched resources (four concurrent clients, warm cache, 10 M-row tables), measured on `kaveon-test-aks` on September 11, 2026." — **claimable with the workload stated.**
- "KaveonDB scans a 504-million-row Parquet table on three 3-CPU workers and answers an exact grouped aggregate in about 20 seconds." — **claimable; not "interactive."**
- "1.9× Trino." — **not claimable.** 66.5 % of the way there.
- "A distributed transactional engine on cloud storage that no one else has." — **not claimable.** Snowflake Unistore, TiDB X and Databricks LTAP occupy the category; KaveonDB's transactional layer is a typed record protocol, not a general database (see `kaveon-vs-htap-platforms.md`).
- "PostgreSQL-free." — **not claimable** until cutover evidence exists.

## Part 5 — The five things that move the scores

1. **Interactive latency on the 504 M-row table**: wide-aggregate decode cost (3–10× the narrow scan) and `COUNT(DISTINCT)` at 3 M keys. Codex has `3531c68` landing; qualification pending. Moves 1.2 from 5 to 7.
2. **Cold-cache and soak runs of the matched corpus.** Moves 1.1 from 7 to 8 and 1.6 from 6 to 7.
3. **S3 reads.** One connector in `object_store` terms; unblocks the Oracle demo host and any non-Azure customer. Moves 1.4 from 5 to 6.
4. **A client protocol.** Even a PostgreSQL wire façade for read-only SQL would open Tableau, Power BI, dbt and DBeaver. Moves 1.9 from 2 to 6 and changes what the product can be sold as.
5. **Recovery and isolation qualification of the record store**: verified head recovery under interrupted publication, restart soak, point-in-time restore from ADLS versions. Moves 2.x from 4 to 6 and is the gate for retiring PostgreSQL.

---

*Sources: `HANDSHAKE.md` Log rows dated 2026-09-10 and 2026-09-11 (query IDs `3dfd7cc9…`, `112ad163…`, `63a8630e…`, `08948cdc…`, `15ab4798…`); `engine/DISTRIBUTED_EXECUTION_STATUS.md`; `docs/engineering/engine-readiness-qualification.md` (80/100 rubric); `docs/engineering/trino-90-percent-benchmark.md`; `engine/crates/sql/src/{parser,logical_plan}.rs`; Trino 483 documentation; PostgreSQL 18 documentation.*
