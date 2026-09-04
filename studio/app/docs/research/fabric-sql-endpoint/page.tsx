import { Callout, PageHeader, Pager } from "../../../../components/docs/prose";

export const metadata = { title: "Kaveon Engine vs Fabric SQL Endpoint" };

export default function KaveonVsFabricSqlDocs() {
  return <div className="docs-prose">
    <PageHeader eyebrow="Research · Architecture" title="Kaveon Engine vs Fabric SQL analytics endpoint" lead="A precise comparison between Kaveon&rsquo;s self-hostable analytical engine and the managed SQL endpoint attached to a Microsoft Fabric Lakehouse." />
    <Callout type="note">This compares engines and SQL-serving boundaries. Fabric&rsquo;s Spark, pipelines, mirroring, Power BI, and governance are surrounding platform workloads; Kaveon DLM and Studio are likewise above Engine.</Callout>

    <table><thead><tr><th>Dimension</th><th>Kaveon Engine</th><th>Fabric SQL analytics endpoint</th></tr></thead><tbody>
      <tr><td>Delivery</td><td>Self-hostable Rust coordinator/workers; hosted Engine pending</td><td>Microsoft-managed and automatically provisioned with a Lakehouse</td></tr>
      <tr><td>Data</td><td>Registered customer-controlled Parquet/Delta locations</td><td>OneLake Delta tables and supported shortcuts</td></tr>
      <tr><td>SQL</td><td>Focused analytical SQL surface</td><td>Read-oriented T-SQL with DQL and limited DDL</td></tr>
      <tr><td>Writes</td><td>Read path today; optimized ingest target</td><td>Read-only table serving; Fabric Warehouse is the transactional offering</td></tr>
      <tr><td>Operations</td><td>Operator-managed alpha runtime</td><td>Managed Fabric capacity</td></tr>
      <tr><td>BI</td><td>Studio integration target</td><td>Power BI and Direct Lake ecosystem</td></tr>
    </tbody></table>

    <h2>Kaveon&rsquo;s intended distinction</h2>
    <p>Kaveon owns the planner, scheduler, exchanges, operators, and readers and is designed to run against storage controlled by its operator. The Fabric endpoint trades that deployment control for automatic OneLake integration and managed operations. Neither distinction proves faster execution.</p>

    <h2>Current qualification gates</h2>
    <ul>
      <li>Direct ADLS Gen2 reads and complete supported Delta semantics.</li>
      <li>Integrated Entra authorization from Studio through Engine and storage.</li>
      <li>Production metadata, admission, spill, recovery, and observability.</li>
      <li>Identical-workload latency, throughput, resource, and cost evidence.</li>
    </ul>

    <h2>Canonical paper</h2>
    <p>Read the complete scope, decision guide, and Microsoft primary references in <a href="https://github.com/PruthviProdduturi/Kaveon/blob/dev/docs/research/kaveon-engine-vs-fabric-sql-analytics-endpoint.md">Kaveon Engine and Microsoft Fabric SQL Analytics Endpoint</a>.</p>
    <Pager prev={{ href: "/docs/research/trino", title: "Kaveon vs Trino" }} />
  </div>;
}
