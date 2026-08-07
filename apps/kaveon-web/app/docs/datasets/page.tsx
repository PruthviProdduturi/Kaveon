import { PageHeader, Callout, Code, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Semantic Datasets" };

export default function DatasetsDocs() {
  return (
    <div className="docs-prose">
      <PageHeader
        eyebrow="Features"
        title="Semantic Datasets"
        lead="A dataset is a reusable semantic layer over one or more tables — you name the dimensions, metrics, and filters once, and every chart, dashboard, and NL→SQL query builds on them without re-writing SQL."
      />

      <h2>Why a semantic layer</h2>
      <p>
        Raw tables don&rsquo;t know which columns are things to <em>group by</em> and which are things to <em>measure</em>.
        A dataset encodes that intent once: dimensions (categories, dates), metrics (aggregations), and filterable
        columns. Charts then reference the dataset, and Kaveon generates the correct <code>SELECT</code>, <code>JOIN</code>,
        <code>GROUP BY</code>, and aggregation for you.
      </p>

      <h2>Anatomy of a dataset</h2>
      <table>
        <thead><tr><th>Part</th><th>Stored in</th><th>Purpose</th></tr></thead>
        <tbody>
          <tr><td><strong>Fact table + joins</strong></td><td><code>datasets</code></td><td>The base table and any related tables to join</td></tr>
          <tr><td><strong>Dimensions</strong></td><td><code>dataset_dimensions</code></td><td>Group-by columns (strings, dates)</td></tr>
          <tr><td><strong>Metrics</strong></td><td><code>dataset_metrics</code></td><td>Aggregations — e.g. <code>SUM(total)</code>, <code>COUNT(*)</code>, <code>AVG(price)</code></td></tr>
          <tr><td><strong>Columns</strong></td><td><code>dataset_columns</code></td><td>The resolved column list used for filters and previews</td></tr>
        </tbody>
      </table>

      <h2>Creating a dataset</h2>
      <ol>
        <li>Go to <strong>Datasets → + New Dataset</strong> and pick a data source.</li>
        <li>Choose a fact table; add related tables and their join keys if you need more than one.</li>
        <li>Mark each column as a <strong>dimension</strong> or a <strong>metric</strong> (with its aggregation).</li>
        <li>Preview, then save. It&rsquo;s now available to every chart and the NL→SQL engine.</li>
      </ol>

      <h2>Automatic SQL generation</h2>
      <p>
        When a chart requests dimensions and metrics, the <code>query_generator</code> service assembles the star-schema
        SQL — selecting the dimensions, applying each metric&rsquo;s aggregation, generating the <code>JOIN</code>s from the
        dataset definition, and grouping/ordering appropriately.
      </p>
      <Code lang="sql">{`-- "revenue by region" against a dataset with
-- dimension: region   metric: SUM(total) AS revenue
SELECT   d.region, SUM(f.total) AS revenue
FROM     fact_orders f
JOIN     dim_region  d ON d.id = f.region_id
GROUP BY d.region
ORDER BY revenue DESC;`}</Code>
      <Callout type="note">
        Role-playing dimensions (the same lookup used in multiple roles, e.g. order-date vs ship-date) are handled with
        <code> COALESCE</code>-based resolution so each role maps to the right join without duplicating the dataset.
      </Callout>

      <h2>Visibility</h2>
      <p>
        Datasets — like charts and dashboards — carry a visibility level: <strong>draft</strong> (only you),
        <strong> internal</strong> (signed-in users), or <strong>published</strong>. Combined with the role model
        (see <a href="/docs/auth">Auth &amp; RBAC</a>), this controls who can discover and build on each dataset.
      </p>

      <Pager
        prev={{ href: "/docs/dashboards", title: "Dashboards" }}
        next={{ href: "/docs/data-sources", title: "Data Sources" }}
      />
    </div>
  );
}
