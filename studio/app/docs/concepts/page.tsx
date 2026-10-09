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

      <h2>1 · One authority, two data planes</h2>
      <p>
        Kaveon separates durable product state from customer lake data while keeping both under one governed
        platform. KaveonDB is the transactional authority for datasets, charts, dashboards, saved queries,
        DLM definitions, audit records and other product metadata. The analytical plane reads Parquet, Delta
        and supported Iceberg snapshots in place; it does not require copying the source into a proprietary
        warehouse.
      </p>
      <table>
        <thead><tr><th></th><th>KaveonDB transactional plane</th><th>Distributed analytical plane</th></tr></thead>
        <tbody>
          <tr><td><strong>Holds</strong></td><td>Versioned product records, ownership, revisions, audit and transaction state</td><td>Customer tables and immutable source snapshots in object storage</td></tr>
          <tr><td><strong>Execution</strong></td><td>Validate → revision check → commit → audit → recover</td><td>Plan → split → execute on workers → Arrow exchange → merge result</td></tr>
          <tr><td><strong>Storage</strong></td><td>Durable KaveonDB product store, local or ADLS-backed</td><td>ADLS Gen2/local Parquet and Delta; registered catalog definitions</td></tr>
          <tr><td><strong>Consistency</strong></td><td>Immutable revisions, compare-and-set heads and owner-scoped reads</td><td>Version-pinned snapshots, exact statistics and fail-closed pruning</td></tr>
        </tbody>
      </table>
      <p>
        This is why a dashboard update and a billion-row aggregate do not compete for the same execution path:
        transactional writes stay bounded and revisioned, while analytical work fans out across workers and
        returns columnar results.
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

      <h2>4 · Transactional and distributed execution</h2>
      <p>
        Kaveon has two cooperating execution paths. The transactional path protects product state; the
        distributed path executes analytical SQL. The deterministic DLM can answer a third way — from a
        compiled context artifact — when the requested shape is already materialized.
      </p>
      <table>
        <thead><tr><th></th><th>Transactional path</th><th>Distributed analytical path</th><th>Context path</th></tr></thead>
        <tbody>
          <tr><td><strong>Runs</strong></td><td>KaveonDB coordinator and transaction API</td><td>Rust coordinator plus distributed workers</td><td>DLM context service and compiled artifacts</td></tr>
          <tr><td><strong>Work</strong></td><td>Product records, permissions, revisions and audit</td><td>Scans, joins, aggregates, windows, sorting and exchange</td><td>Precomputed totals, dimensions, pairs and sketches</td></tr>
          <tr><td><strong>Storage</strong></td><td>KaveonDB product records</td><td>Cataloged Parquet, Delta and supported Iceberg</td><td>Versioned context in governed product storage</td></tr>
          <tr><td><strong>Entry point</strong></td><td>Studio/API product operations</td><td>Studio SQL Lab, <code>kaveon</code> CLI, or Engine HTTP API</td><td>Studio Chat, DLM API, or CLI <code>.ask</code></td></tr>
        </tbody>
      </table>
      <Callout type="warn">
        These paths are complementary, not interchangeable. KaveonDB is the authority for product transactions;
        the distributed Engine is optimized for analytical reads; DLM context is used only when its version and
        semantic shape are fresh enough to answer safely.
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
        mandatory import. Local filesystem and ADLS Gen2 are qualified paths; supported Iceberg snapshots are
        available through the Engine catalog, while S3 remains a staged connector target. Details live in
        <a href="/docs/engine/storage">Storage &amp; Catalogs</a>.
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

      <Pager prev={{ href: "/docs/quickstart", title: "Quickstart" }} next={{ href: "/docs/architecture", title: "Architecture" }} />
    </div>
  );
}
