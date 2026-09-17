# Kaveon connected architecture

This is the target operating architecture for Kaveon. It shows how a request
travels from Studio to a deterministic context answer, a distributed analytical
query, or a transactional product-record operation. **Implemented**, **Alpha**,
and **Target** labels are intentional: a diagram must not turn a roadmap
boundary into a shipped guarantee.

## System topology

```mermaid
flowchart LR
  User[User / CLI / API client]
  Studio[Kaveon Studio\nNext.js · SQL Lab · dashboards]
  Proxy[Same-origin proxy\nEntra session · CSRF · identity]
  API[FastAPI control plane\npolicy · catalog registration · telemetry]
  DLM[DLM router\ndeterministic intent · context · SQL contract]
  Tx[KaveonDB transaction service\nrevision CAS · typed records · outbox]
  Coord[Engine coordinator\nparse · optimize · stage graph · admission]
  Cat[(Native catalog\nSQLite/WAL today · ADLS head target)]
  W1[Worker 1\nArrow operators]
  W2[Worker 2\nArrow operators]
  WN[Worker N\nArrow operators]
  Exchange{{Authenticated Arrow\nshuffle / broadcast / merge}}
  Lake[(Customer ADLS Gen2\nParquet · Delta · Iceberg)]
  Context[(Immutable DLM artifact\nanswers · value index · routing)]
  Product[(Product records\nKaveonDB revisions)]
  Outbox[(Pre-cutover migration outbox)]
  Telemetry[(Query history\nplans · stages · metrics · audit)]
  Recovery[(Snapshots · checkpoints\nrollback evidence)]

  User --> Studio
  User -->|remote SQL / CLI| Coord
  Studio --> Proxy --> API
  API --> DLM
  DLM -->|context hit| Context
  DLM -->|physical query| Coord
  API --> Tx --> Product
  Tx --> Outbox
  API --> Telemetry
  Coord --> Cat
  Coord --> W1 & W2 & WN
  W1 & W2 & WN <--> Exchange
  W1 & W2 & WN --> Lake
  Coord --> Telemetry
  Cat --> Lake
  Product --> Recovery
  Cat --> Recovery
```

### Plane responsibilities

| Plane | Owns | Does not own |
|---|---|---|
| Experience | Sessions, SQL Lab, dashboards, charts, CLI presentation | Trusted identity asserted by a browser header |
| Control | Authentication, authorization, source registration, DLM routing, query records | Distributed operator execution |
| Context | Compiled answers, value indexes, freshness and clarification state | Arbitrary SQL correctness |
| Transaction | Typed product records, revisions, uniqueness/reference checks, ordered events | A claim of general PostgreSQL compatibility until all gates pass |
| Compute | Parsing, optimization, stages, workers, Arrow exchange, spill and retry | Product UI state or source credentials |
| Data | Customer-owned Parquet/Delta objects and immutable snapshots | Kaveon-owned copies of customer rows |
| Evidence | Metrics, audit, checkpoints, snapshots and rollback receipts | Silent success when evidence is missing |

## Analytical query lifecycle

```mermaid
sequenceDiagram
  participant U as User
  participant S as Studio/CLI
  participant A as API + DLM
  participant C as Coordinator
  participant P as Planner
  participant W as Workers
  participant X as Exchange
  participant L as ADLS lake
  participant H as Query history

  U->>S: Question or SQL
  S->>A: Authenticated request
  alt DLM context hit
    A->>A: Resolve dataset, intent and freshness
    A-->>S: Precomputed answer + visualization contract
  else SQL execution
    A->>C: SQL + principal + catalog/schema
    C->>P: Parse, validate, optimize
    P->>P: Exact statistics, pruning, join and aggregate strategy
    P-->>C: Versioned stage graph and fragments
    C->>W: Admit bounded tasks
    W->>L: Footer/log metadata and ranged columnar reads
    L-->>W: RecordBatches
    W->>X: Partitioned intermediate batches
    X-->>W: Repartitioned/broadcast batches
    W-->>C: Final batches and stage metrics
    C->>H: Query, plan, task, resource and result evidence
    C-->>A: Bounded result + query ID
    A-->>S: Rows, columns, timings and visual contract
  end
```

The correctness invariant is: every worker receives a coordinator-resolved
snapshot and fragment; pruning can remove data only when metadata proves it is
irrelevant; exchange retries are idempotent; a failed or unavailable stage
fails the query rather than returning a partial result. Context answers bypass
the scan only when the artifact is ready and fresh.

## Transaction lifecycle

```mermaid
sequenceDiagram
  participant Client as Studio/API
  participant T as Transaction service
  participant K as KaveonDB catalog
  participant O as Outbox
  participant R as Recovery store
  participant M as Migration verifier

  Client->>T: Begin with principal and snapshot
  T->>K: Read owner-scoped revision
  Client->>T: Typed create/update/delete
  T->>T: Validate types, references, uniqueness and bounds
  T->>K: Prepare immutable document and compare-and-swap head
  T->>O: Append ordered canonical event
  T->>R: Record receipt/checkpoint
  T-->>Client: Commit receipt or conflict
  M->>O: Replay from watermark
  M->>K: Exact count, ownership, reference and digest reconciliation
  M-->>Client: Evidence report
```

A transaction receipt is not evidence of PostgreSQL retirement. The runtime has
direct typed KaveonDB mutations for product-record families and does not fall
back to PostgreSQL once a family is cut over. Context cache is deliberately
rebuilt, and legacy AI configuration requires an explicit delete-or-secret-store
disposition. Retirement still requires all 16 families to reconcile, zero
outbox lag, a write fence, shadow-read parity, restart recovery, backup/restore,
rollback, and a verified PostgreSQL-unavailable rehearsal. Until those live
gates pass, PostgreSQL remains the deployment authority and rollback source.

## Storage and recovery boundaries

```mermaid
flowchart TB
  Source[Customer lake snapshot\nADLS Gen2 objects]
  Catalog[Catalog head\nobject identity + revision]
  Txn[Typed KaveonDB records]
  CP[Durable migration checkpoint\nsource watermark + digest]
  Snap[Backup / restore snapshot]
  Audit[Audit + query telemetry]
  Source --> Catalog --> Txn
  Txn --> CP
  Txn --> Snap
  Txn --> Audit
  CP -->|resume only after digest validation| Txn
  Snap -->|rollback rehearsal| Catalog
```

ADLS object publication is create-only and must be reconciled byte-for-byte.
Checkpoints are bounded, integrity-checked and resumable. A missing or corrupt
evidence artifact is a failed gate, never an implicit pass.

## Current versus target boundary

| Capability | Current position | Target acceptance evidence |
|---|---|---|
| DLM context | Retirement mode compiles bounded state from Engine scans, publishes one immutable ADLS artifact, and serves its verified routing, values and answers without PostgreSQL fallback | Freshness, ambiguity, and answer parity corpus on every release and live AKS artifact qualification |
| Distributed SQL | Coordinator/workers, binder, streamed Arrow exchange, columnar aggregates with spill, joins, retry with forced-worker-loss evidence, admission queue, result cache; three benchmark tiers running (`../qualification/benchmark-program.md`) | Completed benchmark rounds, spill under skew, adaptive planning, sustained soak |
| Transactions | Typed product-record protocol with revisions and checkpointed migration tooling | General row DML, isolation, durable WAL-equivalent recovery and crash testing |
| Lake storage | Parquet, Delta (v1 checkpoints) and Iceberg on ADLS Gen2 with workload identity; S3 implemented and unqualified; plain Parquet directories not yet tables | Multi-format snapshot qualification and object-store performance evidence |
| PostgreSQL replacement | Direct reads/mutations, 16-family replay and reconciliation, durable ADLS checkpoints, write fencing and strict evidence runners are implemented; live retirement is not yet qualified | One fresh immutable AKS run passes every gate, then PostgreSQL is scaled to zero while its PVC/snapshot remain through the rollback window |
| Operations | Bicep/Helm deployment materials, immutable image references and bounded rehearsal tooling | Recreate-from-zero on a clean subscription and verified live backup/restore |

The architecture is deliberately honest: Kaveon can be a unified product with
one user experience while its analytical and transactional paths mature at
different rates. The integration contract is explicit so a faster query path
does not silently weaken transactional correctness.
