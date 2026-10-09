import { Callout, PageHeader, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Connector Capability Matrix" };

export default function ConnectorDocs() {
  return <div className="docs-prose">
    <PageHeader eyebrow="Platform" title="Connector matrix" lead="Registration, query execution, and deterministic context profiling are separate capabilities." />
    <table><thead><tr><th>Source</th><th>Studio</th><th>Execution</th><th>DLM boundary</th></tr></thead><tbody>
      <tr><td>Fabric SQL / Azure SQL</td><td>Current</td><td>Current · pyodbc + Azure identity</td><td>Per-dataset DLM compilation; no statistics/HLL profiling</td></tr>
      <tr><td>PostgreSQL</td><td>Current</td><td>Current · psycopg2</td><td>Per-dataset DLM compilation; no database-wide statistics profiling</td></tr>
      <tr><td>StarRocks</td><td>Current</td><td>Current · MySQL protocol</td><td>Per-dataset DLM compilation; no statistics/HLL profiling</td></tr>
      <tr><td>MySQL / MariaDB</td><td>API only</td><td>Current · pymysql</td><td>Not in Studio’s source picker</td></tr>
      <tr><td>Trino</td><td>Registration only</td><td>Target</td><td>No driver</td></tr>
      <tr><td>Parquet, Delta, Iceberg in storage</td><td>Current · registered as a catalog, not a connector</td><td>Current · the Engine, read in place</td><td>Statistics and cube cells per table</td></tr>
    </tbody></table>
    <Callout type="note">Each platform query targets one selected SQL source. Cross-source federation is not implemented. The data-source test endpoint is currently a stub; use SQL Lab or a setup/admin probe for a real connection check.</Callout>
    <p>
      A connection string is never returned by the API, and it is never stored as written. On the current path the
      string goes to the deployment&rsquo;s secret store and only a reference to it is kept on the record; on the
      legacy path it is encrypted with a versioned key, and registration refuses with <code>503</code> rather than
      storing anything if no keyring is configured. A catalog source keeps a credential reference — a Key Vault URI —
      or no credential at all, where the host&rsquo;s own managed or workload identity is what reaches the storage.
    </p>
    <Pager prev={{ href: "/docs/api-reference", title: "API Reference" }} next={{ href: "/docs/sql-lab", title: "SQL Lab" }} />
  </div>;
}
