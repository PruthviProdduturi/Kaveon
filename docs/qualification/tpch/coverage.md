# TPC-H coverage on the Kaveon Engine

The twenty-two TPC-H statements of `trino-queries.sql` (v3.0.1 validation
parameters, Trino dialect; suite `kaveon-suite.json`), as the Engine's SQL
layer, binder, planner and executor take them today. This is the SQL,
planning and execution surface on a tiny synthetic dataset, not a timing
record: SF100 was generated as Delta tables on 2026-09-17 (`tables.json`,
`benchmark-program.md` Tier 3) and no pass over it has been run on the
cluster yet. The gate
`engine/crates/server/src/tpch_coverage.rs` generates a deterministic
eight-table TPC-H (Trino's column names and types — bigint keys, integer
for `p_size`/`l_linenumber`/`o_shippriority`/`ps_availqty`, double for money
and quantities, varchar text, date for dates; a few hundred rows shaped so
that every statement's filters find rows) as Parquet, registers it in a
`MemoryCatalog`, and runs each statement through parse (lowered for the
binder) → `qualify_tables` → `binder::bind` → `push_filter_down` →
`push_projection_down` → the node-local planner to completion, and
separately through the distributed stage planner (`build_stage_graph` +
`build_executable_fragments`, two workers) with the fragments executed in
the test process as two workers would run them
(`differential_tests::execute_distributed`: every stage's tasks in
dependency order, exchange outputs routed as the orchestrator routes them).
A second test computes the answers to Q1, Q2, Q4, Q6, Q13, Q17, Q18, Q20,
Q21 and Q22 independently from the generator's rows and compares them cell
by cell with the Engine's on both paths.

The gate holds this record both ways: a statement outside
`KNOWN_UNSUPPORTED` must parse, plan, execute and return rows, and its
fragments must return as many rows; one inside it must still fail.

## Status at `ac6fd7a` (2026-09-17)

**22 of 22 parse, bind, plan, execute and return rows locally; all 22 plan
and execute as fragments.** At `0f307b1` earlier the same day: 21 of 22 (Q21
refused at parse), fragments planned but not executed. Before this work
(dev `76f1a5b`): 1 of 22 (Q19).

| Query | Parsed | Bound + planned | Executed | Rows | Distributed | Answer checked | Note |
|---|---|---|---|---|---|---|---|
| Q1 | yes | yes | yes | 6 | yes | yes | `DATE - INTERVAL '90' DAY` |
| Q2 | yes | yes | yes | 4 | yes | yes | five-way comma join; correlated scalar `min(ps_supplycost)` decorrelated into a join grouped by `ps_partkey` |
| Q3 | yes | yes | yes | 1 | yes | — | three-way comma join |
| Q4 | yes | yes | yes | 2 | yes | yes | correlated `EXISTS` as a semi join on `o_orderkey = l_orderkey`; `+ INTERVAL '3' MONTH` |
| Q5 | yes | yes | yes | 1 | yes | — | six-way comma join |
| Q6 | yes | yes | yes | 1 | yes | yes | integer and decimal literals against double columns; decimal arithmetic `0.06 - 0.01` |
| Q7 | yes | yes | yes | 1 | yes | — | six-way join with `nation n1, nation n2`; `EXTRACT(YEAR ...)`; derived table |
| Q8 | yes | yes | yes | 2 | yes | — | eight-way join; `sum(CASE ...) / sum(volume)` |
| Q9 | yes | yes | yes | 45 | yes | — | six-way join; `LIKE '%green%'` |
| Q10 | yes | yes | yes | 4 | yes | — | four-way join, `LIMIT 20` |
| Q11 | yes | yes | yes | 16 | yes | — | uncorrelated scalar subquery in HAVING |
| Q12 | yes | yes | yes | 1 | yes | — | `IN ('MAIL', 'SHIP')`, `CASE` sums |
| Q13 | yes | yes | yes | 11 | yes | yes | `LEFT OUTER JOIN ... ON ... AND o_comment NOT LIKE ...`; the non-equality filters the non-preserved side before the join |
| Q14 | yes | yes | yes | 1 | yes | — | `LIKE 'PROMO%'` inside `CASE` |
| Q15 | yes | yes | yes | 1 | yes | — | derived table joined with a comma; uncorrelated scalar `max(total_revenue)` over a derived table |
| Q16 | yes | yes | yes | 9 | yes | — | `NOT IN (subquery)`, `NOT LIKE`, `IN (list)`, `count(DISTINCT ...)` |
| Q17 | yes | yes | yes | 1 | yes | yes | correlated scalar `0.2 * avg(l_quantity)` decorrelated into a join grouped by `l_partkey` |
| Q18 | yes | yes | yes | 6 | yes | yes | `IN (... GROUP BY ... HAVING sum(...) > 300)`; HAVING aggregates are computed by the aggregate |
| Q19 | yes | yes | yes | 1 | yes | — | three-way `OR` of conjunctions over a two-table join |
| Q20 | yes | yes | yes | 2 | yes | yes | nested `IN`; correlated scalar on two columns (`l_partkey = ps_partkey AND l_suppkey = ps_suppkey`) decorrelated into a two-key join |
| Q21 | yes | yes | yes | 1 | yes | yes | correlated `EXISTS` and `NOT EXISTS` on `l_orderkey` with `l_suppkey <> l1.l_suppkey` as the semi and anti join's residual; see below |
| Q22 | yes | yes | yes | 1 | yes | yes | `substr`, uncorrelated scalar `avg(c_acctbal)`, correlated `NOT EXISTS` as an anti join |

"Distributed" means `build_stage_graph` and `build_executable_fragments`
accept the bound plan with two workers and the fragments, executed in the
test process as two workers would run them, return as many rows as the
node-local plan (and, for the ten checked answers, the same cells). The
fragments have not been run on a cluster in this pass (no AKS access was
used).

## What changed, in the order it unblocked queries

Each is one commit with tests in the crate that owns it; the gate's
`KNOWN_UNSUPPORTED` list moved with every commit.

| Commit | Change | Coverage |
|---|---|---|
| `ec6cebb` | the gate itself (`tpch_coverage.rs`) | 1 / 22 (baseline) |
| `24482d9` | **sql** — `date ± INTERVAL 'n' DAY` is day-number arithmetic; `MONTH`/`YEAR` intervals shift the calendar of a DATE literal at lowering time (day clamped to the target month's end, as Trino does) | 3 / 22 |
| `9f60ce2` | **core** — an integer or decimal literal against a Float64 column reads as a double in storage predicates (`l_quantity < 24`, `c_acctbal > 0.00`) | — |
| `8f885a8` | **exec** — arithmetic between two decimals stays decimal (`0.06 - 0.01`) | 4 / 22 |
| `147f151` | **exec, server** — one column resolver (exact, else the unique bare-name match) for the projection operator, the planner's aggregate column resolution and the hash partitioner; join keys keep their qualifiers | 7 / 22 |
| `78c91be` | **optim** — the binder (`kaveon_optim::binder`): scopes from the catalog, bare columns under a join qualified with their relation, comma joins with WHERE equalities turned into hash joins, projection pruning routed through nested joins; hooked into both API pipelines after `qualify_tables` | 12 / 22 |
| `aa5f4db` | **sql** — HAVING aggregates the projection does not select are still computed (`GROUP BY k HAVING sum(x) > 300` lowered to a DISTINCT before) | 13 / 22 |
| `0be8863` | **optim** — ON conditions route like WHERE; an outer join filters its non-preserved side by the ON's single-side conjuncts and refuses residuals with the offending conjunct | 14 / 22 |
| `6a5ec64` | **optim** — Filter/Sort/Window asked for every column ask their input for every column (they pruned to their own columns) | — |
| `5aab4b5` | **sql** — uncorrelated scalar subqueries in WHERE and HAVING as single-row cross joins (`__kaveon_scalar_n`); the subquery must be an ungrouped aggregate | 16 / 22 |
| `30c9c89` | **sql** — an expression over a simple aggregate (`0.2 * avg(x)`) lowers the aggregate to a named column | — |
| `5e1ad6b` | **optim** — a semi join prunes its left by what is required plus its key; the subquery side prunes by its own projection | — |
| `9a14a20` | **sql, server** — a clippy lint and a fragment test the two preceding sql commits owed | — |
| `56dd22e` | **optim** — decorrelation: a subquery binds with the enclosing scope behind its own; a WHERE equality with the enclosing query rides up through the subquery's projection (extra column) and aggregate (group key) to the joining node and becomes its key — `EXISTS`/`NOT EXISTS` as semi/anti join keys, correlated scalar aggregates as inner joins on the grouped aggregate | 21 / 22 |
| `0f307b1` | **test** — data shaped so every statement finds rows; nine answers checked against an independent computation | 21 / 22 |
| `26b1487` | **exec** — `SemiJoinOperator::with_residual`: the build side keeps its rows by key (only the residual's columns), each probe row's pairs are gathered and the residual evaluated over them in 8192-pair chunks, a left row matching when any pair holds; EXISTS's NULL rules under a residual; the right key may be named beside the residual's columns | — |
| `87ea380` | **sql, optim, server, cli** — `SemiJoin`/`AntiJoin` carry `residual: Option<Expr>` through pushdown, pruning, statistics and the binder to both planners (`JoinSpec.residual`, broadcast placement unchanged); `sql_to_logical_plan_for_binder` lowers a subquery's outer reference in place for the binder (both API pipelines, both gates), `sql_to_logical_plan` keeps refusing it (the CLI's unbound planner) | — |
| `bb85af2` | **optim** — the binder decorrelates `EXISTS`/`NOT EXISTS` with residual conjuncts: the first equality is the key, further equalities and any other conjunct the residual, the subquery projecting the residual's columns beside the key under fresh names; refused by name under an aggregate, at a scalar subquery's join, over a HAVING, across two enclosing queries | 22 / 22 |
| `ac6fd7a` | **server** — `differential_tests::execute_distributed` runs a stage graph in-process as two workers; the differential sweep compares dictionary, plain and fragments (thirty cases, two of Q21's shape), the TPC-H gate executes all 22 as fragments and checks ten answers on both paths; a fragment aggregate resolving `GROUP BY t.country` by exact name over `events t` fixed | — |

## Q21 — how it plans

```sql
... WHERE ... AND EXISTS (SELECT * FROM lineitem l2
                          WHERE l2.l_orderkey = l1.l_orderkey AND l2.l_suppkey <> l1.l_suppkey)
             AND NOT EXISTS (SELECT * FROM lineitem l3
                          WHERE l3.l_orderkey = l1.l_orderkey AND l3.l_suppkey <> l1.l_suppkey
                            AND l3.l_receiptdate > l3.l_commitdate) ...
```

The SQL layer, lowering for the binder, leaves `l1.l_orderkey` and
`l1.l_suppkey` in the subqueries' filters. The binder binds each subquery
with the enclosing query's scope behind its own and lifts both conjuncts
that reference it: `l2.l_orderkey = l1.l_orderkey` is the key correlation,
`l2.l_suppkey <> l1.l_suppkey` a residual one (its subquery side
`l2.l_suppkey` projected out as `__kaveon_corr_0`). `l3.l_receiptdate >
l3.l_commitdate` references only the subquery and stays its own filter,
pushed to the scan. The plan:

```
Limit 100
  Sort numwait DESC, s_name
    Project s_name, numwait
      Aggregate GROUP BY supplier.s_name, count(*)
        AntiJoin   key l1.l_orderkey = l3.l_orderkey   residual __kaveon_corr_1 <> l1.l_suppkey
        ├─ SemiJoin key l1.l_orderkey = l2.l_orderkey  residual __kaveon_corr_0 <> l1.l_suppkey
        │  ├─ Join(inner, supplier.s_nationkey = nation.n_nationkey)
        │  │  ├─ Join(inner, l1.l_orderkey = orders.o_orderkey)
        │  │  │  ├─ Join(inner, supplier.s_suppkey = l1.l_suppkey)
        │  │  │  │  ├─ Scan supplier [s_name, s_nationkey, s_suppkey]
        │  │  │  │  └─ Filter l_receiptdate > l_commitdate ← Scan lineitem l1 [4 columns]
        │  │  │  └─ Filter o_orderstatus = 'F' ← Scan orders [o_orderkey, o_orderstatus]
        │  │  └─ Filter n_name = 'SAUDI ARABIA' ← Scan nation [n_name, n_nationkey]
        │  └─ Project l2.l_orderkey, l2.l_suppkey AS __kaveon_corr_0 ← Scan lineitem l2 [l_orderkey, l_suppkey]
        └─ Project l3.l_orderkey, l3.l_suppkey AS __kaveon_corr_1
             ← Filter l_receiptdate > l_commitdate AND l_orderkey IS NOT NULL ← Scan lineitem l3 [4 columns]
```

Distributed, both subquery projections broadcast into the probe stage
(`JoinSpec { join_type: Semi | Anti, broadcast: true, residual: Some(…) }`),
as keyed semi joins already did. The executor builds the subquery side as
rows by key holding only the residual's columns, gathers each probe row's
pairs and evaluates the residual over them in bounded chunks; a probe row
passes the semi join when any pair holds and the anti join when none does.
On the gate's data (every fifth order kept waiting by the SAUDI ARABIA
supplier) the answer is one row, `Supplier#000000003 | 30`, checked cell by
cell against an independent computation on both paths.

## What remains for Q21 at scale

- The subquery side is broadcast whole to every probe task: at SF100 that
  is two projections of `lineitem` (key and supplier) per worker. A
  partitioned semi join on the key — both sides hash-exchanged on
  `l_orderkey` — is the next step; the operator does not change.
- The retained build rows have no spill: a build side over the query's
  budget fails closed.
- `NOT EXISTS` on a NULL probe key without a residual still takes NOT IN's
  rule (dropped rather than kept); the residual path takes EXISTS's. TPC-H
  keys are never NULL.

## Other limits the binder reports by name

- `COUNT` in a correlated scalar subquery (its result for an unmatched row
  is zero, which no join row carries — the rewrite would need an outer join
  plus `COALESCE`).
- `LIMIT`/`OFFSET` or a set operation inside a correlated subquery.
- A correlated `EXISTS` with no equality at all (only non-equalities): a
  nested-loop semi join, refused rather than run as one.
- A correlated predicate other than an equality under an aggregate, at a
  scalar subquery's join, over a HAVING, or referencing two enclosing
  queries; correlated `IN`.
- A scalar subquery that is not an ungrouped aggregate.
- `INTERVAL MONTH`/`YEAR` added to a column (only DATE literals shift at
  lowering time; a runtime calendar function would be needed).
- A bare column that names two relations of a self-join (`n_name` beside
  `nation n1, nation n2`) is refused as ambiguous, as SQL requires.
