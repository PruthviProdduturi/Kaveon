# SQL and execution

## What runs today

The Engine executes scans, filters, projections, arithmetic (integer,
double and decimal), comparisons through the dictionary when a column has
one, aliases, grouped and global aggregates (COUNT/SUM/MIN/MAX/AVG, exact
`COUNT(DISTINCT …)`, `SUM(DISTINCT)`/`AVG(DISTINCT)`), `GROUP BY` with no
aggregate (as DISTINCT), HAVING with aggregates the projection does not
select, `ORDER BY` with `NULLS FIRST/LAST`, `LIMIT`, `OFFSET`
(`ORDER BY … OFFSET n LIMIT m` is a top-N), row `DISTINCT`, inner, left,
right, full and cross joins, semi and anti joins from `IN`/`NOT IN`/
`EXISTS`/`NOT EXISTS`, `UNION [ALL]`, `INTERSECT`, `EXCEPT`, CTEs, derived
tables, uncorrelated scalar subqueries in WHERE and HAVING, correlated
`EXISTS`/`NOT EXISTS` and correlated scalar aggregates decorrelated by the
binder, window functions (`ROW_NUMBER`, `RANK`, `DENSE_RANK`, `LAG`, `LEAD`
and aggregate windows with `ROWS`/`RANGE`/`GROUPS` frames), `CASE`,
`COALESCE`, `BETWEEN`, `IN` lists, `LIKE`/`ILIKE` through Arrow's kernels,
`REGEXP_REPLACE`, `CAST`, string concatenation and functions
(`UPPER`, `LOWER`, `LENGTH`, `TRIM`, `CONCAT`, `SUBSTR`/`SUBSTRING`, `REPEAT`, `REPLACE`, `LPAD`, `RPAD`),
`EXTRACT`, `DATE_TRUNC`, `DATE_PART`, `TO_CHAR`, `NOW`, `CURRENT_DATE`,
`CURRENT_TIMESTAMP`, `DATE 'YYYY-MM-DD'` literals, `date ± INTERVAL 'n'
DAY` (and `MONTH`/`YEAR` against a DATE literal), and Decimal128 literals
and arithmetic. Every shape runs locally and, where the stage planner can
express it, distributed; the coordinator records in `execution.detail` why
it ran a shape itself.

Coverage records: TPC-H 21 of 22 statements parse, bind, plan and execute
with a distributed plan (`docs/qualification/tpch/coverage.md`; the gate is
`cargo test -p kaveon-server tpch`), and every ClickBench statement has run
on the AKS cluster (`docs/qualification/clickbench-2026-09-16.md`). The
differential sweep (`server/src/differential_tests.rs`, 28 statement shapes
over two Parquet encodings of the same rows) runs under `cargo test`.

## The binder

`kaveon_optim::binder` runs after table qualification in both API statement
pipelines. It builds scopes from the catalog, qualifies bare columns under a
join with their relation (an ambiguous bare name in a self-join is refused),
turns comma joins with WHERE equalities into hash joins, routes ON
conditions like WHERE (an outer join filters its non-preserved side by the
ON's single-side conjuncts and refuses residuals by name), routes projection
pruning through nested joins, and decorrelates: a WHERE equality with the
enclosing query rides up through the subquery's projection and aggregate to
the joining node and becomes its key. An unbindable statement is refused as
an analysis error. The CLI's embedded local mode does not bind; comma joins
there plan as cross products.

## Refused by name

Unsupported syntax fails explicitly rather than returning a wrong answer:
correlated non-equality subqueries (TPC-H Q21: `l2.l_suppkey <>
l1.l_suppkey` needs a semi join with a residual over matched pairs, which
`SemiJoinOperator` and `JoinSpec.residual` do not carry yet), `COUNT` in a
correlated scalar subquery, `LIMIT`/`OFFSET` or set operations inside a
correlated subquery, `EXISTS` correlated on more than one column, correlated
`IN`, a scalar subquery that is not an ungrouped aggregate, `INTERVAL
MONTH`/`YEAR` added to a column, recursive CTEs, `GROUPING SETS`/`CUBE`/
`ROLLUP`, `INTERSECT ALL`/`EXCEPT ALL`, named windows, window `FILTER`/
`WITHIN GROUP`/`DISTINCT`, approximate aggregates, array/map/JSON types,
general DDL and row DML (the product transaction API is a bounded
product-record protocol, see the [SQL compatibility
reference](../reference/engine-sql-compatibility.md)).

## Definition of complete

A feature requires parser semantics, binding, logical planning, safe
optimization, null/type/error-correct physical execution, versioned fragment
support where applicable, local/distributed equivalence, tests, and matching
documentation. Kaveon does not claim an ANSI SQL percentage without a
published conformance corpus.
