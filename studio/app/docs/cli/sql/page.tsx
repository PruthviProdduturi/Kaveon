import { Code, PageHeader, Pager } from "../../../../components/docs/prose";

export const metadata = { title: "SQL and catalog administration — Kaveon CLI" };

export default function CliSqlDocs() {
  return <div className="docs-prose">
    <PageHeader eyebrow="CLI" title="SQL and catalog administration" lead="Query lake data and, with the appropriate role, register and optimize the metadata that makes it fast." />
    <h2>Query</h2>
    <Code lang="sql">{`SELECT region, count(*) AS n
FROM OpenSource.public.events
GROUP BY region
ORDER BY n DESC
LIMIT 10;`}</Code>
    <h2>Register a lake table</h2>
    <Code lang="sql">{`CREATE CATALOG IF NOT EXISTS Analytics WITH (
  storage = 'adls', account = 'kaveonlake', container = 'opensource',
  root = 'snapshots/current', credential = 'workload-identity:kaveon-reader'
);
CREATE SCHEMA IF NOT EXISTS Analytics.sales;
CREATE TABLE IF NOT EXISTS Analytics.sales.orders WITH (
  location = 'sales/orders', format = 'parquet'
);`}</Code>
    <h2>Measure and optimize</h2>
    <Code lang="sql">{`ANALYZE Analytics.sales.orders WITH (sketches = true);
SHOW STATS FOR Analytics.sales.orders;
DESCRIBE DETAIL Analytics.sales.orders;
OPTIMIZE Analytics.sales.orders;`}</Code>
    <p>Registration reads the table metadata in place; it does not copy rows. The full SQL compatibility matrix and lifecycle rules remain in the <a href="https://github.com/PruthviProdduturi/Kaveon/blob/dev/docs/guides/engine-cli.md">CLI reference</a>.</p>
    <Pager prev={{ href: "/docs/cli/commands", title: "Interactive commands" }} next={{ href: "/docs/cli/automation", title: "Automation & output" }} />
  </div>;
}
