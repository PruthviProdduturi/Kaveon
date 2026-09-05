import { PageHeader, Callout, Code, Diagram, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Core concepts" };

export default function ConceptsDocs() {
  return (
    <div className="docs-prose">
      <PageHeader
        eyebrow="Getting Started"
        title="Core concepts"
        lead="Six ideas explain how Kaveon fits together. Once these click, the rest of the documentation is detail."
      />

      <h2>1 · Two planes: metadata and analytical data</h2>
      <p>
        Kaveon keeps its own state separate from the data it queries. This split is why you can point Kaveon
        at a warehouse without it needing to own or copy anything in it.
      </p>
      <table>
        <thead><tr><th></th><th>Metadata database</th><th>Data sources</th></tr></thead>
        <tbody>
          <tr><td><strong>Holds</strong></td><td>Datasets, charts, dashboards, query history, DLM context, roles, themes</td><td>Your analytical data — the tables you actually query</td></tr>
          <tr><td><strong>Configured via</strong></td><td>Setup wizard, or <code>Settings → Metadata Server</code></td><td>The <a href="/docs/data-sources">Data Sources</a> page</td></tr>
          <tr><td><strong>How many</strong></td><td>Exactly one</td><td>As many as you register</td></tr>
        </tbody>
      </table>
      <p>
        In the reference deployment these are two databases on one server: <code>kaveonmeta</code> for the
        control plane and context, and <code>kaveon</code> as a warehouse. Separating them keeps context
        lookups fast while large scans run elsewhere.
      </p>

      <h2>2 · The content chain</h2>
      <p>Each layer is reusable by the next, and each one exists so you stop repeating yourself:</p>
      <Code lang="text">{`data source ──▶ dataset ──▶ chart ──▶ dashboard
 connection      meaning     one view    many views
                                          + filters`}</Code>
      <ul>
        <li><strong>Data source</strong> — a connection to a database (<a href="/docs/data-sources">docs</a>).</li>
        <li><strong>Dataset</strong> — a semantic layer over tables: which columns are dimensions, which are metrics, which is time (<a href="/docs/datasets">docs</a>).</li>
        <li><strong>Chart</strong> — a visualization bound to a dataset (<a href="/docs/charts">docs</a>).</li>
        <li><strong>Dashboard</strong> — a canvas of charts sharing filters (<a href="/docs/dashboards">docs</a>).</li>
      </ul>

      <h2>3 · A dataset encodes intent once</h2>
      <p>
        This is the concept that pays for itself. A dataset says what your columns <em>mean</em>, so nothing
        downstream has to restate it. Define it once:
      </p>
      <Code lang="text">{`dataset  orders
  table       public.orders
  dimensions  region, plan
  metrics     Revenue = SUM(total)
              Orders  = COUNT(*)
  time        ordered`}</Code>
      <p>
        Now &ldquo;Revenue by region&rdquo; is fully specified — as a chart, as a dashboard tile, or as a
        question typed in English. Kaveon assembles the aggregation, grouping, joins, and time filtering for
        you, in the dialect of the source it is talking to:
      </p>
      <Code lang="sql">{`SELECT region, SUM(total) AS "Revenue"
FROM   public.orders
GROUP  BY region
ORDER  BY "Revenue" DESC`}</Code>
      <Callout type="tip">
        You never hand-write JOINs or GROUP BYs for charts. Drop into <a href="/docs/sql-lab">SQL Lab</a>
        when you want raw control — the two paths coexist, and SQL Lab results can be saved back as datasets.
      </Callout>

      <h2>4 · Two execution paths</h2>
      <p>
        Kaveon has two ways to execute analytical work, and they are currently separate systems. Knowing
        which one you are using explains most of what you will see.
      </p>
      <table>
        <thead><tr><th></th><th>Platform path</th><th>Engine path</th></tr></thead>
        <tbody>
          <tr><td><strong>Runs</strong></td><td>Studio and the FastAPI service</td><td>The Rust Engine, standalone</td></tr>
          <tr><td><strong>Reads</strong></td><td>Registered SQL sources — PostgreSQL, Fabric SQL, Azure SQL, MySQL, StarRocks</td><td>Parquet and Delta in storage, through catalogs</td></tr>
          <tr><td><strong>Speaks</strong></td><td>Each source&rsquo;s own SQL dialect</td><td>Kaveon Engine SQL</td></tr>
          <tr><td><strong>Entry point</strong></td><td>Studio, or the platform API</td><td><code>kaveon</code> CLI, or the Engine HTTP API</td></tr>
          <tr><td><strong>Maturity</strong></td><td>Shipping</td><td>Alpha</td></tr>
        </tbody>
      </table>
      <Callout type="warn">
        Studio does not route queries through the Engine yet. Everything you do in the UI today goes through
        the platform path. The Engine is used directly, on its own.
      </Callout>

      <h2>5 · Catalogs — how the Engine sees storage</h2>
      <p>
        Where the platform path has data sources, the Engine has <strong>catalogs</strong>. A catalog points
        at storage and the tables inside it are addressed as{" "}
        <code>catalog.schema.table</code>. Tables in a local catalog are discovered from the{" "}
        <code>.parquet</code> files present:
      </p>
      <Code lang="toml">{`# ~/.kaveon/catalogs/warehouse.toml — the filename becomes the catalog name
type      = "local"
base_path = "/data/warehouse"`}</Code>
      <Code lang="sql">{`SHOW CATALOGS;
USE warehouse.default;
SELECT count(*) FROM orders;`}</Code>
      <p>
        The direction here is the <strong>Live Lake Path</strong>: read data where it already lives, with no
        mandatory import. Local filesystem works today; ADLS Gen2, S3, and Iceberg are target work, tracked
        in <a href="/docs/engine/storage">Storage &amp; Catalogs</a>.
      </p>

      <h2>6 · Ask, don&rsquo;t query</h2>
      <Diagram
        src="/docs/architecture/kaveon-intelligence-loop.svg"
        alt="Kaveon intelligence loop separating deterministic DLM and analytical compute paths"
        caption="Natural-language resolution and analytical execution are complementary paths. Engine integration into the shipping Studio request path is target architecture."
      />
      <p>
        Questions are resolved by the <strong>DLM</strong> — a compiled per-dataset context artifact — not by
        a hosted language model. Compiling a dataset precomputes each metric&rsquo;s total, its breakdown per
        dimension, and low-cardinality dimension pairs, so common questions are answered without touching the
        database at all. Kaveon always tells you which path answered:
      </p>
      <table>
        <thead><tr><th>Badge</th><th>Meaning</th><th>Cost</th></tr></thead>
        <tbody>
          <tr><td>From context</td><td>Served from precomputed context</td><td>No database scan</td></tr>
          <tr><td>From sketch · ≈</td><td>Approximate distinct count from a HyperLogLog sketch</td><td>No database scan</td></tr>
          <tr><td>Live query · Xs</td><td>Shape was not precomputed; SQL was assembled and run</td><td>One source query</td></tr>
        </tbody>
      </table>
      <p>
        Because resolution is rule-based, the same question produces the same SQL every time — reproducible,
        inspectable, and free of token cost. See <a href="/docs/nl-to-sql">DLM · NL→SQL</a> for how routing
        decides, and <a href="/docs/freshness">Freshness</a> for when context is preferred over a live read.
      </p>

      <h2>Access: two independent axes</h2>
      <p>
        <strong>Roles</strong> gate what you can <em>do</em>: <code>Viewer → Analyst → Editor → Admin</code>.{" "}
        <strong>Visibility</strong> gates who can <em>see</em> a given object: <code>private</code>,{" "}
        <code>internal</code>, <code>published</code>. They compose — an Editor still cannot read someone
        else&rsquo;s private dashboard.
      </p>
      <p>
        The API defines all four roles, but OAuth sign-in resolves a user to just two of them:{" "}
        <strong>Admin</strong> if their email is listed in <code>AUTH_ADMIN_EMAILS</code>, otherwise{" "}
        <strong>Viewer</strong>. Full model in <a href="/docs/auth">Auth &amp; RBAC</a>.
      </p>

      <Pager
        prev={{ href: "/docs/quickstart", title: "Quickstart" }}
        next={{ href: "/docs/architecture", title: "Architecture" }}
      />
    </div>
  );
}
