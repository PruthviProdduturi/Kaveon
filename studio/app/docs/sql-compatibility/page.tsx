import { Callout, PageHeader, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Engine SQL Compatibility" };

export default function SqlCompatibilityDocs() {
  return <div className="docs-prose">
    <PageHeader eyebrow="Engine · Alpha" title="SQL compatibility" lead="The standalone Rust Engine implements a deliberately small executable SQL surface over local Parquet. Parser acceptance alone does not mean a feature is physically executed." />
    <Callout type="warn"><strong>ORDER BY is not enforced.</strong> It is parsed into a Sort plan, but both physical planners currently pass that node through.</Callout>
    <h2>Current executable surface</h2>
    <table><thead><tr><th>Feature</th><th>State</th><th>Boundary</th></tr></thead><tbody>
      <tr><td>SELECT and projection</td><td>Alpha</td><td>Strict local-Parquet columns</td></tr>
      <tr><td>WHERE</td><td>Alpha</td><td>Comparisons and boolean expressions; no planner-to-storage pushdown yet</td></tr>
      <tr><td>GROUP BY</td><td>Alpha</td><td>Blocking in-memory hash aggregate</td></tr>
      <tr><td>SUM, COUNT, AVG, MIN, MAX</td><td>Alpha</td><td>No spill or distributed partial aggregation</td></tr>
      <tr><td>LIMIT</td><td>Alpha</td><td>Physical limit operator</td></tr>
      <tr><td>ORDER BY</td><td>Parsed only</td><td>No physical Sort or TopN operator</td></tr>
    </tbody></table>
    <h2>Not implemented</h2>
    <p>Joins, subqueries, windows, HAVING, set operations, DDL/DML, cloud/table-format reads, distributed fragments, spill, and cancellation are not executable. Engine statement execution is synchronous and materializes the complete JSON result.</p>
    <p>The canonical source-controlled matrix is <a href="https://github.com/PruthviProdduturi/Kaveon/blob/dev/docs/reference/engine-sql-compatibility.md">Engine SQL Compatibility</a>.</p>
    <Pager prev={{ href: "/docs/engine", title: "Kaveon Engine" }} next={{ href: "/docs/connectors", title: "Connector Matrix" }} />
  </div>;
}
