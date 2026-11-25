import { Callout, Code, PageHeader } from "../../../components/docs/prose";

export const metadata = { title: "Kaveon CLI" };

export default function CliDocs() {
  return (
    <div className="docs-prose">
      <PageHeader
        eyebrow="Command line"
        title="Kaveon CLI"
        lead="A terminal client with a live session header, streaming result pages, query cancellation, catalog completion and scriptable output."
      />

      <Callout type="note">The remote shell is Beta. Embedded <code>--local</code> mode is a preview path separate from the qualified Docker runtime. The full reference covers every option, dot command, SQL keyword, catalog/schema/table lifecycle command, optimization clause, output field and unsupported boundary in the <a href="https://github.com/PruthviProdduturi/Kaveon/blob/dev/docs/guides/engine-cli.md">CLI guide</a>.</Callout>

      <h2>Install once</h2>
      <Code lang="powershell">{`irm https://raw.githubusercontent.com/PruthviProdduturi/Kaveon/dev/scripts/install.ps1 | iex
$env:PATH = "$env:LOCALAPPDATA\\kaveon\\bin;$env:PATH"
kaveon --version`}</Code>
      <Code lang="bash">{`curl -fsSL https://raw.githubusercontent.com/PruthviProdduturi/Kaveon/dev/scripts/install.sh | bash
kaveon --version`}</Code>
      <p>Tagged releases include platform archives and SHA256SUMS. The same client runs on Windows x64, macOS Intel and Apple Silicon, and Linux x64.</p>

      <h2>Connect with context</h2>
      <Code lang="bash">{`kaveon https://engine.example.com/OpenSource/nyc_taxi
kaveon --server https://engine.example.com --catalog OpenSource --schema nyc_taxi`}</Code>
      <p>Connection defaults can live in <code>~/.kaveon_config</code>. Command-line flags override them. The client records <code>--user</code>, <code>--source</code> and <code>--client-tags</code> as query metadata; they never grant access.</p>

      <h2>Connect to an AKS installation</h2>
      <p>Connect to the private coordinator using your installation’s cluster name and trusted CA. The <a href="/docs/cli/install">install and connect guide</a> includes the port-forward and CLI commands.</p>

      <h2>Authentication</h2>
      <p><code>auto</code> uses an access token when supplied, then Azure CLI, then Microsoft device sign-in when the coordinator advertises Entra. <code>azure-cli</code> requires the current Azure login, <code>microsoft</code> starts device sign-in explicitly, and <code>none</code> is for loopback development only. Tokens remain in process memory.</p>

      <h2>A shell built for long queries</h2>
      <ul>
        <li>Session header shows Engine version, environment, worker health, admission and authenticated role.</li>
        <li>Emacs-style editing, history navigation, reverse search, inline history suggestions and lazy SQL/catalog completion.</li>
        <li>Live submitting, queued, running and cancellation states with task count, workers, rows scanned and admission wait.</li>
        <li>Paged results arrive while a streaming query runs; <code>Space</code> or <code>Enter</code> advances pages and <code>q</code> stops paging.</li>
        <li><code>Ctrl-C</code> cancels the coordinator query; a second press returns to the editor immediately.</li>
      </ul>
      <Code lang="text">{`kaveon › SELECT region, count(*) FROM Kaveon.usage.events GROUP BY region;
 ⠸ Running 2.4 s · 3/5 tasks · 2 workers · 210M rows scanned`}</Code>

      <h2>Inspect execution</h2>
      <p><code>EXPLAIN ANALYZE</code> prints the optimized plan, phase timings, stages, task counters, memory, exchange bytes, rows scanned and spill. The shell reports coordinator-local, distributed-fragment and context-answer paths distinctly.</p>

      <h2>Scripts and output</h2>
      <p>Use <code>-e</code>, <code>-f</code> or redirected stdin for automation. <code>--ignore-errors</code> continues a batch while preserving a failing exit status. Choose aligned tables, vertical rows, CSV, TSV, JSON or JSONL; <code>--row-limit</code>, <code>--width</code>, <code>--pager</code> and <code>--no-header</code> keep output deterministic for CI.</p>

      <h2>Catalog and session commands</h2>
      <Code lang="sql">{`.catalogs
.schemas OpenSource
.tables
.use OpenSource.nyc_taxi
.queries
.kill <query-id>
.format JSONL
exit`}</Code>
      <p>SQL metadata statements and dot commands are kept separate: dot commands never get sent to the SQL parser. The CLI can list granted catalogs, schemas, tables and columns, switch context, inspect recent queries and stop a running query.</p>

      <h2>Register and optimize lake tables</h2>
      <p>Administrators can create a catalog; analysts or administrators can create schemas and register existing Parquet, Delta, or Iceberg tables. Registration probes the location before activation and never copies the rows.</p>
      <Code lang="sql">{`CREATE CATALOG IF NOT EXISTS Analytics WITH (
  storage = 'adls', account = 'kaveonlake', container = 'opensource',
  root = 'snapshots/2026-09-09-v1',
  credential = 'workload-identity:kaveon-reader'
);
CREATE SCHEMA IF NOT EXISTS Analytics.sales;
CREATE TABLE IF NOT EXISTS Analytics.sales.orders WITH (
  location = 'sales/orders', format = 'parquet',
  partitioned_by = ARRAY['order_date'], clustered_by = ARRAY['customer_id'],
  bloom = ARRAY['order_id']
);
ANALYZE Analytics.sales.orders WITH (sketches = true);
OPTIMIZE Analytics.sales.orders;`}</Code>
      <p>Use <code>ALTER TABLE … SET CLUSTERED BY</code> and <code>SET SHAPE</code> before a cube build, then inspect <code>SHOW STATS</code>, <code>DESCRIBE DETAIL</code> and <code>SHOW CREATE TABLE</code>. The detailed keyword matrix and lifecycle rules are in the <a href="https://github.com/PruthviProdduturi/Kaveon/blob/dev/docs/guides/engine-cli.md#sql-keywords-and-query-patterns">CLI reference</a>.</p>
    </div>
  );
}
