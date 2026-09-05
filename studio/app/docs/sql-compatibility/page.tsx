import { Callout, PageHeader, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Engine SQL Compatibility" };

export default function SqlCompatibilityDocs() {
  return <div className="docs-prose">
    <PageHeader
      eyebrow="Engine · Alpha"
      title="SQL compatibility"
      lead="The executable surface of the standalone Rust Engine, feature by feature. Parser acceptance alone does not mean a feature is physically executed — this table tracks what actually runs."
    />

    <Callout type="note">
      This covers the <strong>standalone Engine only</strong>. SQL that Studio sends to a registered
      PostgreSQL, Fabric SQL, Azure SQL, MySQL, or StarRocks source is governed by that database — see{" "}
      <a href="/docs/connectors">Connector Matrix</a>.
    </Callout>

    <h2>Executable today</h2>
    <table><thead><tr><th>Feature</th><th>Local</th><th>Distributed</th><th>Notes</th></tr></thead><tbody>
      <tr><td><code>SELECT</code>, projection, aliases</td><td>Yes</td><td>Yes</td><td>Strict — unknown or duplicate columns fail</td></tr>
      <tr><td><code>WHERE</code></td><td>Yes</td><td>Yes</td><td>Conservative predicate pushdown to storage; residual filters retained for correctness</td></tr>
      <tr><td><code>GROUP BY</code></td><td>Yes</td><td>Yes</td><td>Hash aggregate; partial/final across workers</td></tr>
      <tr><td><code>SUM</code> <code>COUNT</code> <code>AVG</code> <code>MIN</code> <code>MAX</code></td><td>Yes</td><td>Yes</td><td><code>AVG</code> merges as a weighted state</td></tr>
      <tr><td><code>COUNT(DISTINCT)</code>, <code>SUM/AVG(DISTINCT)</code></td><td>Yes</td><td>Yes</td><td>Exact mergeable distinct state</td></tr>
      <tr><td><code>HAVING</code></td><td>Yes</td><td>Yes</td><td>Post-aggregation filter</td></tr>
      <tr><td><code>ORDER BY</code>, <code>NULLS FIRST/LAST</code></td><td>Yes</td><td>Yes</td><td>Multi-key with per-key direction; bounded external merge</td></tr>
      <tr><td><code>LIMIT</code> / TopN</td><td>Yes</td><td>Yes</td><td><code>LIMIT</code> above a sort fuses into TopN</td></tr>
      <tr><td>Equi joins — inner, left, right, full</td><td>Yes</td><td>Yes</td><td>Distributed via hash repartition</td></tr>
      <tr><td>Cross joins</td><td>Yes</td><td>Yes</td><td>Broadcast build side</td></tr>
      <tr><td>Window functions</td><td>Yes</td><td>Yes</td><td><code>ROW_NUMBER</code> <code>RANK</code> <code>DENSE_RANK</code> <code>LAG</code> <code>LEAD</code>, aggregates with <code>OVER</code></td></tr>
      <tr><td>Set operations</td><td>Yes</td><td>Yes</td><td><code>UNION ALL</code>, <code>INTERSECT</code>, <code>EXCEPT</code></td></tr>
      <tr><td>Date and time functions</td><td>Yes</td><td>Yes</td><td><code>EXTRACT</code> <code>DATE_TRUNC</code> <code>DATE_PART</code> <code>TO_CHAR</code> <code>NOW</code> <code>CURRENT_DATE</code> <code>CURRENT_TIMESTAMP</code></td></tr>
      <tr><td>Conditional and comparison</td><td>Yes</td><td>Yes</td><td><code>CASE</code> <code>COALESCE</code> <code>BETWEEN</code> <code>IN</code> <code>LIKE</code> <code>ILIKE</code> <code>CAST</code></td></tr>
      <tr><td>Qualified names</td><td>Yes</td><td>Yes</td><td><code>catalog.schema.table</code></td></tr>
    </tbody></table>

    <h2>Not executable</h2>
    <ul>
      <li>Scalar and correlated subqueries.</li>
      <li>Non-equality join conditions — equalities only, though general predicates work in <code>WHERE</code>.</li>
      <li>DDL and DML. The Engine reads; it does not create, insert, update, or delete.</li>
      <li>Comprehensive decimal and date/time edge-case semantics.</li>
    </ul>

    <h2>Storage boundaries</h2>
    <p>
      SQL support is independent of what the Engine can read. Local Parquet and local Delta work today; Delta
      requires a complete JSON commit history from version 0, with no checkpoint replay. ADLS Gen2, S3, and
      Iceberg are not readable yet, so no SQL feature reaches them. See{" "}
      <a href="/docs/engine/storage">Storage &amp; Catalogs</a>.
    </p>

    <h2>Execution boundaries</h2>
    <p>
      <code>POST /v1/statement</code> is synchronous and materializes the full root result before
      serializing. Query history is process-local. Admission control, resource groups, aggregate and join
      spill, cost-based optimization, dynamic filtering, and exchange streaming flow control remain open —
      Sort and TopN are the only operators with bounded spill today. See{" "}
      <a href="/docs/memory">Engine Memory</a>.
    </p>

    <Callout type="warn">
      Unsupported syntax stays unsupported even when the upstream parser accepts it. The executable contract
      is the intersection of parsing, logical planning, and physical operator construction — statements can
      parse and still fail at planning time.
    </Callout>

    <Pager prev={{ href: "/docs/memory", title: "Engine Memory" }} next={{ href: "/docs/deployment", title: "Deployment" }} />
  </div>;
}
