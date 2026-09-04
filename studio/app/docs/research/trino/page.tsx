import { Callout, PageHeader, Pager } from "../../../../components/docs/prose";

export const metadata = { title: "Kaveon vs Trino" };

export default function KaveonVsTrinoDocs() {
  return <div className="docs-prose">
    <PageHeader eyebrow="Research · Architecture" title="Kaveon vs Trino" lead="An evidence-bounded comparison of Kaveon&rsquo;s integrated Engine, DLM, and Studio architecture with Trino&rsquo;s mature distributed SQL engine." />
    <Callout type="warn"><strong>No benchmark claim:</strong> Kaveon has not demonstrated general performance superiority over Trino. Comparative results require identical data, SQL, hardware, configuration, cache state, concurrency, and result validation.</Callout>

    <h2>The essential difference</h2>
    <p>Trino is a production-mature distributed SQL federation engine. Kaveon is building one data-intelligence product in which a Rust analytical Engine, deterministic Data Language Model, and BI Studio share a contract. The overlap is distributed analytical SQL; the product boundaries are not the same.</p>

    <table><thead><tr><th>Dimension</th><th>Kaveon</th><th>Trino</th></tr></thead><tbody>
      <tr><td>Core runtime</td><td>Rust and Arrow <code>RecordBatch</code></td><td>JVM coordinator and workers</td></tr>
      <tr><td>Product boundary</td><td>Engine + DLM + Studio</td><td>Distributed SQL engine</td></tr>
      <tr><td>Catalog</td><td>Native single-coordinator durable catalog; adapters are targets</td><td>Connector catalogs and established external integrations</td></tr>
      <tr><td>Distributed execution</td><td>Executable alpha stages, tasks, splits, exchanges, joins, aggregates, and TopN</td><td>Mature stages, tasks, drivers, splits, exchanges, and scheduling</td></tr>
      <tr><td>Optimization</td><td>Projection/filter pushdown and Parquet pruning; no CBO or dynamic filtering</td><td>Statistics-driven CBO, connector pushdown, and dynamic filtering</td></tr>
      <tr><td>SQL coverage</td><td>Focused alpha subset; no CTEs, subqueries, HAVING, general SELECT DISTINCT, windows, or CASE</td><td>Substantially broader mature SQL surface</td></tr>
      <tr><td>Language intelligence</td><td>Deterministic DLM above Engine</td><td>Outside the engine boundary</td></tr>
    </tbody></table>

    <h2>What Kaveon must still prove</h2>
    <ul>
      <li>ADLS Gen2 execution and broader Delta protocol correctness.</li>
      <li>Admission control, aggregate/join spill, durable exchange, and sustained failure recovery.</li>
      <li>End-user Engine authentication, authorization, TLS, quotas, and governance integration.</li>
      <li>Reproducible performance and cost results against the same Trino workload.</li>
    </ul>

    <h2>Canonical paper</h2>
    <p>The full comparison, decision guide, proof standard, and primary references live in <a href="https://github.com/PruthviProdduturi/Kaveon/blob/dev/docs/research/kaveon-vs-trino.md">Kaveon and Trino: Architectural Comparison</a>.</p>
    <Pager prev={{ href: "/docs/research", title: "Papers & Patents" }} next={{ href: "/docs/research/fabric-sql-endpoint", title: "Fabric SQL endpoint" }} />
  </div>;
}
