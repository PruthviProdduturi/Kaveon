# TPC-H coverage on the Kaveon Engine

The twenty-two TPC-H statements of `trino-queries.sql` (v3.0.1 validation
parameters, Trino dialect; suite `kaveon-suite.json`), as the Engine's SQL
layer, binder, planner and executor take them today. This is the SQL and
planning surface, not a timing record: SF100 was generated as Delta tables
on 2026-09-17 (`tables.json`, `benchmark-program.md` Tier 3) and no pass over
it has been run on the cluster yet. The gate
`engine/crates/server/src/tpch_coverage.rs` generates a deterministic
eight-table TPC-H (Trino's column names and types — bigint keys, integer
for `p_size`/`l_linenumber`/`o_shippriority`/`ps_availqty`, double for money
and quantities, varchar text, date for dates; a few hundred rows shaped so
that every statement's filters find rows) as Parquet, registers it in a
`MemoryCatalog`, and runs each statement through parse → `qualify_tables` →
`binder::bind` → `push_filter_down` → `push_projection_down` → the
node-local planner to completion, and separately through the distributed
stage planner (`build_stage_graph` + `build_executable_fragments`, two
workers). A second test computes the answers to Q1, Q2, Q4, Q6, Q13, Q17,
Q18, Q20 and Q22 independently from the generator's rows and compares them
cell by cell with the Engine's.

The gate holds this record both ways: a statement outside
`KNOWN_UNSUPPORTED` must parse, plan, execute and return rows; one inside it
must still fail.

## Status at `0f307b1` (2026-09-17)

**21 of 22 parse, bind, plan, execute and return rows locally; all 21 have a
distributed plan.** Before this work (dev `76f1a5b`): 1 of 22 (Q19).

| Query | Parsed | Bound + planned | Executed | Rows | Distributed plan | Answer checked | Note |
|---|---|---|---|---|---|---|---|
| Q1 | yes | yes | yes | 6 | yes | yes | `DATE - INTERVAL '90' DAY` |
| Q2 | yes | yes | yes | 4 | yes | yes | five-way comma join; correlated scalar `min(ps_supplycost)` decorrelated into a join grouped by `ps_partkey` |
| Q3 | yes | yes | yes | 1 | yes | — | three-way comma join |
| Q4 | yes | yes | yes | 2 | yes | yes | correlated `EXISTS` as a semi join on `o_orderkey = l_orderkey`; `+ INTERVAL '3' MONTH` |
| Q5 | yes | yes | yes | 1 | yes | — | six-way comma join |
| Q6 | yes | yes | yes | 1 | yes | yes | integer and decimal literals against double columns; decimal arithmetic `0.06 - 0.01` |
| Q7 | yes | yes | yes | 1 | yes | — | six-way join with `nation n1, nation n2`; `EXTRACT(YEAR ...)`; derived table |
| Q8 | yes | yes | yes | 2 | yes | — | eight-way join; `sum(CASE ...) / sum(volume)` |
| Q9 | yes | yes | yes | 44 | yes | — | six-way join; `LIKE '%green%'` |
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
| Q21 | **no** | — | — | — | no | — | see below |
| Q22 | yes | yes | yes | 1 | yes | yes | `substr`, uncorrelated scalar `avg(c_acctbal)`, correlated `NOT EXISTS` as an anti join |

"Distributed plan" means `build_stage_graph` and `build_executable_fragments`
accept the bound plan with two workers; the fragments have not been run on
a cluster in this pass (no AKS access was used).

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

## Q21 — what remains and what it needs

```sql
... WHERE ... AND EXISTS (SELECT * FROM lineitem l2
                          WHERE l2.l_orderkey = l1.l_orderkey AND l2.l_suppkey <> l1.l_suppkey)
             AND NOT EXISTS (SELECT * FROM lineitem l3
                          WHERE l3.l_orderkey = l1.l_orderkey AND l3.l_suppkey <> l1.l_suppkey
                            AND l3.l_receiptdate > l3.l_commitdate) ...
```

Today it fails at parse: `correlated subqueries are unsupported: l1.l_orderkey`.
Two things stand between it and a plan:

1. **The SQL layer refuses a qualified outer reference before the binder
   sees it.** `validate_uncorrelated` rejects any qualified column whose
   qualifier is not a relation of the subquery. That guard exists because a
   plan that reaches the executor unbound would resolve `l1.l_orderkey`
   against `l2`'s own `l_orderkey` by bare-name match and return a wrong
   answer silently (`rejects_correlated_subqueries_instead_of_rebinding_outer_columns`
   pins this). Bare outer references pass the guard and the binder
   decorrelates them (Q2, Q4, Q17, Q20, Q22). Lifting the guard is safe only
   where the binder is guaranteed to run — both API pipelines — so the
   lowering would need a mode (or the guard would move into the binder and
   the unbound CLI planner would refuse correlated plans itself).

2. **The semi join carries one equality key and no residual.**
   `l2.l_suppkey <> l1.l_suppkey` is a correlated *non-equality*. The
   decorrelation rewrite would be: semi (anti) join on `l_orderkey` with a
   residual predicate evaluated per matched pair — `EXISTS` holds when any
   matching `l2` row satisfies `l2.l_suppkey <> l1.l_suppkey`, `NOT EXISTS`
   when no matching `l3` row satisfies its residual. That needs a hash semi
   join that keeps the build side's rows per key (today
   `SemiJoinOperator` keeps a set of keys), probes each left row, evaluates
   the residual over the (left row, right row) pairs and emits the left row
   on any (semi) or no (anti) pass — locally in `exec/src/semijoin.rs`, and
   for the distributed path `JoinSpec.residual`, which
   `fragment_exec.rs` today refuses as "residual fragment join filters are
   not implemented". The binder already lifts the correlation and reports
   the non-equality with its text; it would hand the residual to the new
   operator instead.

## Other limits the binder reports by name

- `COUNT` in a correlated scalar subquery (its result for an unmatched row
  is zero, which no join row carries — the rewrite would need an outer join
  plus `COALESCE`).
- `LIMIT`/`OFFSET` or a set operation inside a correlated subquery.
- `EXISTS` correlated on more than one column; correlated `IN`.
- A scalar subquery that is not an ungrouped aggregate.
- `INTERVAL MONTH`/`YEAR` added to a column (only DATE literals shift at
  lowering time; a runtime calendar function would be needed).
- A bare column that names two relations of a self-join (`n_name` beside
  `nation n1, nation n2`) is refused as ambiguous, as SQL requires.
