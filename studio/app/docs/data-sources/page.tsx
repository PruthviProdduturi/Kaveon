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
        <thead><tr><th>Database</th><th>Availability</th><th>Driver</th></tr></thead>
        <tbody>
          <tr><td>Microsoft Fabric SQL</td><td>Studio + API · <code>fabric_sql</code></td><td>pyodbc · ODBC Driver 18</td></tr>
          <tr><td>Azure SQL</td><td>Studio + API · <code>azure_sql</code></td><td>pyodbc · ODBC Driver 18</td></tr>
          <tr><td>PostgreSQL</td><td>Studio + API · <code>postgresql</code></td><td>psycopg2</td></tr>
          <tr><td>MySQL</td><td>API only · <code>mysql</code></td><td>pymysql</td></tr>
          <tr><td>StarRocks</td><td>Studio + API · <code>starrocks</code></td><td>pymysql (MySQL protocol)</td></tr>
          <tr><td>Local Parquet</td><td>Engine alpha</td><td>Arrow · Parquet</td></tr>
          <tr><td>Trino, ADLS Gen2, S3, Delta Lake, Iceberg</td><td>Target</td><td>Not executable</td></tr>
        </tbody>
      </table>

      <h2>Adding a data source</h2>
      <Callout type="note">Requires the <strong>Admin</strong> role.</Callout>
      <p>
        Go to <strong>Data Sources → + Add Data Source</strong>, fill in a unique name, the type, database name,
        connection string, region (<code>WW</code> / <code>EU</code> for residency tracking), and an optional description.
        Click <strong>Test Connection</strong>, then <strong>Save</strong>.
      </p>
      <Callout type="note">MySQL is implemented in the API but is not currently exposed in Studio&rsquo;s source-type picker. StarRocks is exposed separately and uses the MySQL wire protocol.</Callout>

      <h3>Connection string formats</h3>
      <Code lang="text">{`Fabric SQL   <workspace>.database.fabric.microsoft.com     (Azure AD Managed Identity)
Azure SQL    <server>.database.windows.net                 (Azure AD Managed Identity)
PostgreSQL   postgresql://user:pass@host:5432/db?sslmode=require
MySQL        mysql://user:pass@host:3306/db
StarRocks    mysql://user:pass@host:9030/db`}</Code>
      <p>
        Fabric and Azure SQL carry <strong>no credentials in the string</strong> — auth is via Azure AD
        (<code>DefaultAzureCredential</code>). PostgreSQL enforces SSL by default (Neon/Supabase URLs work as-is).
        StarRocks speaks the MySQL wire protocol, so it connects through the same <code>pymysql</code> path as MySQL
        rather than a distinct driver.
      </p>

      <h2>Testing connections</h2>
      <Callout type="warn">The registered-source <code>POST /data-sources/{`{id}`}/test</code> endpoint is currently a stub and must not be treated as proof of connectivity. First-run setup probes perform real checks, but full connector-specific testing is still a product gap.</Callout>
      <p>Until the endpoint is implemented, verify a source with a least-privilege read through SQL Lab and confirm schema discovery separately. Do not infer write permission from a successful read.</p>

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
