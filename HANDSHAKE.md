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
| `core` — shared types, errors, traits | Shared (either can add, neither restructures without updating this doc) | Scaffold |
| `storage` — Parquet reader, ADLS Gen 2 | **Codex** | Not started |
| `exec/scan` — scan operator | **Claude** | Scaffold |
| `exec/aggregate` — hash aggregate | **Claude** | Scaffold |
| `exec/filter` — filter evaluation | **Claude** | Scaffold |
| `exec/sort` — sort operator | **Codex** | Not started |
| `exec/topn` — TopN operator | **Codex** | Not started |
| `sql` — parser, logical plan | **Claude** | Scaffold |
| `optim` — filter pushdown | **Codex** | Not started |
| `python` — PyO3 bindings | **Claude** | Scaffold |
| `benches` — Criterion benchmarks | **Codex** | Not started |

### API (`api/`)

| Area | Owner | Status |
|------|-------|--------|
| DLM engine (`api/dlm/`) | **Claude** | Done (shipping) |
| DLM standalone API extraction | **Claude** | Not started |
| Routers, services, middleware | **Claude** | Done (shipping) |

### Studio (`studio/`)

| Area | Owner | Status |
|------|-------|--------|
| Platform rebrand (about, landing, nav) | **Claude** | Not started |
| Lakehouse data source UI | **Claude** | Not started |
| UX polish | **Claude** | Not started |

### Launch

| Area | Owner | Status |
|------|-------|--------|
| README rewrite | **Claude** | Not started |
| Docker Compose | **Claude** | Not started |
| Demo datasets | **Claude** | Not started |
| White papers | **Claude** | Not started |
| Architecture diagrams | **Claude** | Not started |

---

## Interface contracts

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
    Scan { table, columns },
    Filter { input, predicate },
    Project { input, columns },
    Aggregate { input, group_by, aggregates },
    Sort { input, order_by },
    Limit { input, count },
}
```

- **Claude** owns the SQL parser and logical plan
- **Claude** owns the physical plan translation (logical → physical operators)
- **Codex** owns the optimizer pass (rewrite logical plan before physical translation)

### Engine → API boundary (PyO3)

```python
import kaveon_engine

result = kaveon_engine.execute("SELECT ...", "/path/to/data")
version = kaveon_engine.version()
```

- **Claude** owns PyO3 bindings
- Exposes `execute(sql, data_path) -> dict` and `version() -> str`

### DLM API (standalone)

- **Claude** extracts DLM into callable API endpoints independent of Studio
- OpenAPI spec at `api/dlm/openapi.yaml`
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
