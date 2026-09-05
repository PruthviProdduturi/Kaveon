import { Callout, Code, PageHeader, Pager } from "../../../../components/docs/prose";

export const metadata = { title: "Engine SQL" };

export default function EngineSqlDocs() {
  return <div className="docs-prose">
    <PageHeader
      eyebrow="Engine Manual"
      title="SQL and execution"
      lead="What Kaveon Engine SQL can execute today, with the standard it holds itself to: a feature counts only when parsing, semantics, physical execution, distributed fragments, tests, and documentation all agree."
    />

    <Callout type="note">
      This page describes the <strong>standalone Engine</strong>. SQL that Studio sends to a registered
      PostgreSQL, Fabric, or MySQL source is governed by that database, not by this page — see{" "}
      <a href="/docs/connectors">Connector Matrix</a>.
    </Callout>

    <h2>Queries and projection</h2>
    <p>
      Projection is strict: unknown or duplicated columns are an error rather than a silent pass-through.
      Tables resolve as <code>catalog.schema.table</code>, or unqualified against the session default.
    </p>
    <Code lang="sql">{`SELECT region, plan, total
FROM   warehouse.default.orders
WHERE  total > 500 AND region <> 'Asia'
ORDER  BY total DESC NULLS LAST
LIMIT  20;`}</Code>
    <p>
      <code>ORDER BY</code> supports multiple keys, per-key direction, and explicit null placement. A{" "}
      <code>LIMIT</code> directly above a sort is fused into a TopN operator rather than sorting the whole
      input.
    </p>

    <h2>Aggregation</h2>
    <Code lang="sql">{`SELECT   region,
         count(*)                AS orders,
         sum(total)              AS revenue,
         avg(total)              AS avg_order,
         count(DISTINCT plan)    AS plans,
         sum(DISTINCT total)     AS distinct_total
FROM     orders
GROUP    BY region
HAVING   sum(total) > 1000
ORDER    BY revenue DESC;`}</Code>
    <p>
      <code>SUM</code>, <code>COUNT</code>, <code>AVG</code>, <code>MIN</code>, <code>MAX</code> and{" "}
      <code>COUNT(DISTINCT …)</code> all execute, locally and distributed. Distributed aggregation is
      partial/final: workers emit partial states that the coordinator merges, so <code>AVG</code> is carried
      as a weighted state and <code>DISTINCT</code> as an exact mergeable state rather than being recomputed.
    </p>

    <h2>Joins</h2>
    <Code lang="sql">{`SELECT c.region, count(*) AS orders
FROM   orders o
JOIN   customers c ON o.customer_id = c.id
GROUP  BY c.region;`}</Code>
    <p>
      <code>INNER</code>, <code>LEFT</code>, <code>RIGHT</code>, <code>FULL</code> and <code>CROSS</code>{" "}
      joins execute with qualified relation aliases. Distributed equi-joins repartition both sides by hash;
      cross joins broadcast the build side. Join conditions must be equalities — general predicates are not
      yet supported as join conditions, though they work in <code>WHERE</code>.
    </p>

    <h2>Window functions</h2>
    <Code lang="sql">{`SELECT region,
       plan,
       total,
       row_number() OVER (PARTITION BY region ORDER BY total DESC) AS rank_in_region,
       lag(total)   OVER (PARTITION BY region ORDER BY ordered)    AS prev_total,
       sum(total)   OVER (PARTITION BY region)                     AS region_total
FROM   orders;`}</Code>
    <p>
      <code>ROW_NUMBER</code>, <code>RANK</code>, <code>DENSE_RANK</code>, <code>LAG</code> and{" "}
      <code>LEAD</code> are available, as is any supported aggregate used with <code>OVER</code>, with{" "}
      <code>PARTITION BY</code> and <code>ORDER BY</code>.
    </p>

    <h2>Set operations</h2>
    <Code lang="sql">{`SELECT region FROM orders_2025
UNION ALL
SELECT region FROM orders_2026;

SELECT region FROM orders INTERSECT SELECT region FROM targets;
SELECT region FROM orders EXCEPT    SELECT region FROM excluded;`}</Code>

    <h2>Expressions and functions</h2>
    <table>
      <thead><tr><th>Group</th><th>Available</th></tr></thead>
      <tbody>
        <tr><td>Conditional</td><td><code>CASE WHEN … THEN … ELSE … END</code>, <code>COALESCE</code></td></tr>
        <tr><td>Comparison</td><td><code>BETWEEN</code>, <code>IN (…)</code>, <code>LIKE</code>, <code>ILIKE</code>, <code>IS [NOT] NULL</code></td></tr>
        <tr><td>Numeric</td><td>arithmetic with numeric coercion, <code>ROUND</code>, <code>ABS</code></td></tr>
        <tr><td>String</td><td><code>UPPER</code>, <code>LOWER</code>, <code>SUBSTRING</code>, concatenation</td></tr>
        <tr><td>Date and time</td><td><code>EXTRACT</code>, <code>DATE_TRUNC</code>, <code>DATE_PART</code>, <code>TO_CHAR</code>, <code>NOW</code>, <code>CURRENT_DATE</code>, <code>CURRENT_TIMESTAMP</code></td></tr>
        <tr><td>Casting</td><td><code>CAST(x AS type)</code></td></tr>
      </tbody>
    </table>
    <Code lang="sql">{`SELECT date_trunc('month', ordered) AS month,
       extract(year FROM ordered)   AS yr,
       upper(region)                AS region,
       CASE WHEN total > 500 THEN 'large' ELSE 'small' END AS bucket
FROM   orders
WHERE  ordered >= current_date - 90;`}</Code>

    <h2>Not executable yet</h2>
    <ul>
      <li>Scalar and correlated subqueries.</li>
      <li>Non-equality join conditions.</li>
      <li>DDL and DML — the Engine reads; it does not create or mutate tables.</li>
      <li>Comprehensive decimal and date/time edge-case behavior.</li>
    </ul>
    <Callout type="warn">
      Treat unsupported syntax as unsupported even when the upstream parser accepts it. The executable
      contract is the intersection of parsing, logical planning, and physical operator construction — a
      statement that parses can still fail at planning.
    </Callout>

    <h2>How capability is claimed</h2>
    <p>
      Kaveon does not publish an ANSI SQL conformance percentage, because it has no conformance corpus to
      back one. A feature is listed here only once its parser, operator, and distributed-fragment path pass
      together on <code>dev</code>. Work in progress in a branch is not a capability claim.
    </p>
    <p>
      To check what your build actually does rather than trusting this page, run the statement — a planning
      failure is explicit:
    </p>
    <Code lang="bash">{`kaveon --local --data-dir /data/warehouse -e "SELECT row_number() OVER (ORDER BY total) FROM orders LIMIT 1"`}</Code>

    <Pager prev={{ href: "/docs/engine/architecture", title: "Engine Architecture" }} next={{ href: "/docs/engine/distributed", title: "Distributed Runtime" }} />
  </div>;
}
