import { Callout, PageHeader, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Connector Capability Matrix" };

export default function ConnectorDocs() {
  return <div className="docs-prose">
    <PageHeader eyebrow="Platform · Current and target" title="Connector matrix" lead="Registration, query execution, and deterministic context profiling are separate capabilities." />
    <table><thead><tr><th>Source</th><th>Studio</th><th>Execution</th><th>DLM boundary</th></tr></thead><tbody>
      <tr><td>Fabric SQL / Azure SQL</td><td>Current</td><td>Current · pyodbc + Azure identity</td><td>No PostgreSQL statistics/HLL profiling</td></tr>
      <tr><td>PostgreSQL</td><td>Current</td><td>Current · psycopg2</td><td>Full implemented profiler path</td></tr>
      <tr><td>StarRocks</td><td>Current</td><td>Current · MySQL protocol</td><td>No PostgreSQL statistics/HLL profiling</td></tr>
      <tr><td>MySQL / MariaDB</td><td>API only</td><td>Current · pymysql</td><td>Not in Studio’s source picker</td></tr>
      <tr><td>Trino</td><td>Registration only</td><td>Target</td><td>No driver</td></tr>
      <tr><td>Local Parquet</td><td>Not a Studio connector</td><td>Alpha Engine</td><td>Separate runtime</td></tr>
    </tbody></table>
    <Callout type="note">Each platform query targets one selected SQL source. Cross-source federation is not implemented. The data-source test endpoint is currently a stub; use SQL Lab or a setup/admin probe for a real connection check.</Callout>
    <p>Connection strings for PostgreSQL/MySQL/StarRocks are suppressed from API responses but remain plaintext in the metadata table. Vault-backed connector-secret storage is target work.</p>
    <Pager prev={{ href: "/docs/sql-compatibility", title: "SQL Compatibility" }} next={{ href: "/docs/troubleshooting", title: "Troubleshooting" }} />
  </div>;
}
