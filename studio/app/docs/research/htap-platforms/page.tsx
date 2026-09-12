import { Callout, PageHeader, Pager } from "../../../../components/docs/prose";

export const metadata = { title: "Kaveon vs HTAP platforms" };

export default function KaveonVsHtapPlatformsDocs() {
  return <div className="docs-prose">
    <PageHeader eyebrow="Research" title="Kaveon vs HTAP platforms" lead="How Kaveon&rsquo;s unified transactional and analytical direction compares with Snowflake Unistore, Databricks LTAP, TiDB X, and Microsoft Fabric." />
    <Callout type="warn"><strong>Not an empty category:</strong> unifying transactional and analytical processing on object storage is contested. Kaveon must not claim it is unprecedented, and must not describe itself as a transactional database today. The SQL layer parses a bounded DML surface and returns an AST; row execution, constraints, isolation, and durable commit are not implemented.</Callout>

    <h2>Market map</h2>

    <table><thead><tr><th>Platform</th><th>Engines</th><th>Durable authority</th><th>Open source</th><th>Status</th></tr></thead><tbody>
      <tr><td>Snowflake Unistore</td><td>One platform, Hybrid Tables</td><td>Proprietary format on object storage</td><td>No</td><td>GA November 2024 on AWS</td></tr>
      <tr><td>Databricks LTAP</td><td><strong>Two</strong>: Postgres (Lakebase) and Spark/Photon</td><td>Object storage, Delta and Iceberg</td><td>Partial</td><td>Announced June 2026, &ldquo;coming soon&rdquo;</td></tr>
      <tr><td>TiDB X</td><td>One</td><td>Object storage as single source of truth</td><td>Apache 2.0</td><td>Announced October 2025; GA unconfirmed</td></tr>
      <tr><td>Fabric SQL database</td><td>SQL plus Spark</td><td>OneLake, read-only Delta mirror for analytics</td><td>No</td><td>GA</td></tr>
      <tr><td><strong>Kaveon</strong></td><td>One (target)</td><td>ADLS Gen2 (target)</td><td>MIT</td><td>Analytics alpha; transactional layer not implemented</td></tr>
    </tbody></table>

    <h2>Where Kaveon differs</h2>
    <p><strong>Data is queried in place.</strong> Kaveon reads customer Parquet and Delta files where they already reside and registers them through the catalog. Snowflake and TiDB require data to be loaded into engine-owned storage; Fabric mirrors into OneLake. For data already in ADLS Gen2, this is the difference between a registration and a migration.</p>
    <p><strong>Natural language is deterministic.</strong> The DLM resolves questions to SQL and precomputed context with no hosted model, so the same question yields the same answer and the derivation is auditable. No platform surveyed provides a deterministic natural-language interface, and TiDB provides no semantic layer at all.</p>

    <h2>Maturity, stated honestly</h2>
    <p>Kaveon is behind every platform in this comparison on transactional maturity. TiDB has approximately a decade of production operation and a CNCF Graduated storage engine; Snowflake Unistore has been generally available for close to two years. Kaveon has a foundation: typed product records with revision-checked writes, uniqueness and reference validation, digest-verified immutable documents, and a conditional head update. Row DML, constraints, isolation, index shard splitting, verified head recovery, and the migration path remain open. PostgreSQL remains the authoritative product store.</p>

    <h2>Canonical paper</h2>
    <p>The full market map, per-platform notes, defensible claim, and primary references live in <a href="https://github.com/PruthviProdduturi/Kaveon/blob/dev/docs/research/kaveon-vs-htap-platforms.md">Kaveon and unified transactional/analytical platforms</a>.</p>
    <Pager prev={{ href: "/docs/research/fabric-sql-endpoint", title: "Engine vs Fabric SQL" }} next={{ href: "/docs/research/scorecard", title: "KaveonDB scorecard" }} />
  </div>;
}
