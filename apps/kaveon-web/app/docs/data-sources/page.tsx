import { PageHeader, Callout, Code, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Data Sources" };

export default function DataSourcesDocs() {
  return (
    <div className="docs-prose">
      <PageHeader
        eyebrow="Features"
        title="Data Sources"
        lead="A data source is a database connection Kaveon can query. You need at least one before creating datasets, charts, or using NL→SQL. Register and test them from the UI — no .env edits, no restart."
      />

      <h2>Supported databases</h2>
      <table>
        <thead><tr><th>Database</th><th>Type</th><th>Driver</th></tr></thead>
        <tbody>
          <tr><td>Microsoft Fabric SQL</td><td><code>fabric_sql</code></td><td>pyodbc · ODBC Driver 18</td></tr>
          <tr><td>Azure SQL</td><td><code>azure_sql</code></td><td>pyodbc · ODBC Driver 18</td></tr>
          <tr><td>PostgreSQL</td><td><code>postgresql</code></td><td>psycopg2</td></tr>
          <tr><td>MySQL</td><td><code>mysql</code></td><td>pymysql</td></tr>
          <tr><td>StarRocks</td><td><code>StarRocks</code></td><td>pymysql (MySQL-compatible)</td></tr>
          <tr><td>Trino</td><td>coming soon</td><td>—</td></tr>
        </tbody>
      </table>

      <h2>Adding a data source</h2>
      <Callout type="note">Requires the <strong>Admin</strong> role.</Callout>
      <p>
        Go to <strong>Data Sources → + Add Data Source</strong>, fill in a unique name, the type, database name,
        connection string, region (<code>WW</code> / <code>EU</code> for residency tracking), and an optional description.
        Click <strong>Test Connection</strong>, then <strong>Save</strong>.
      </p>

      <h3>Connection string formats</h3>
      <Code lang="text">{`Fabric SQL   <workspace>.database.fabric.microsoft.com     (Azure AD Managed Identity)
Azure SQL    <server>.database.windows.net                 (Azure AD Managed Identity)
PostgreSQL   postgresql://user:pass@host:5432/db?sslmode=require
MySQL        mysql://user:pass@host:3306/db
StarRocks    mysql://user:pass@host:9030/db`}</Code>
      <p>
        Fabric and Azure SQL carry <strong>no credentials in the string</strong> — auth is via Azure AD
        (<code>DefaultAzureCredential</code>). PostgreSQL enforces SSL by default (Neon/Supabase URLs work as-is).
      </p>

      <h2>Testing connections</h2>
      <p>On <strong>Test Connection</strong>, the backend opens a fresh (non-pooled) connection, runs <code>SELECT 1</code>, and creates/drops a temp table to verify write access. It returns a structured outcome:</p>
      <table>
        <thead><tr><th><code>error_type</code></th><th>Meaning</th></tr></thead>
        <tbody>
          <tr><td>(none)</td><td>Connectivity and write access both succeeded</td></tr>
          <tr><td><code>access_denied</code></td><td>Connected, but the identity lacks permission</td></tr>
          <tr><td><code>db_not_found</code></td><td>Server reachable, database doesn&rsquo;t exist</td></tr>
          <tr><td><code>timeout</code></td><td>No response within 60s</td></tr>
          <tr><td><code>connection_failed</code></td><td>TCP couldn&rsquo;t connect — check host / firewall</td></tr>
        </tbody>
      </table>

      <h2>Managing sources</h2>
      <p>
        Admins can edit any field; rotate credentials by editing the connection string. Sources can be
        <strong> enabled/disabled</strong> (<code>is_active</code>) — disabled ones are hidden from selectors but keep their
        metadata. Each user can mark one <strong>favorite</strong>, which sorts first, pre-selects on the NL→SQL homepage,
        and appears in the sidebar quick-access.
      </p>

      <Pager
        prev={{ href: "/docs/datasets", title: "Semantic Datasets" }}
        next={{ href: "/docs/architecture", title: "Architecture" }}
      />
    </div>
  );
}
