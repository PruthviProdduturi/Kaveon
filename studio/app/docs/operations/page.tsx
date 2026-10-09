import { Callout, Code, Diagram, PageHeader, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Operations" };

export default function OperationsDocs() {
  return <div className="docs-prose">
    <PageHeader eyebrow="Deploy &amp; Operate" title="Operations" lead="Verifying a Kaveon deployment, protecting its trust boundaries, backing up the two places it keeps its memory, and rebuilding the catalog when a host is replaced." />

    <h2>Know your deployment</h2>
    <p>
      One request path: browser → Studio (Vercel, AKS, or VM) → its same-origin proxy → the API → the Kaveon Engine coordinator →
      its workers → object storage. The Engine is the query path for every chart, dashboard and question; nothing
      answers around it. Studio is the only front door, and the API trusts the proxy header, so the API and the
      Engine must never be published.
    </p>
    <Diagram src="/docs/architecture/kaveon-deployment-topology.svg" alt="Browser to Studio on Vercel, through a same-origin proxy to the API on the host behind Caddy, into the Engine coordinator and two workers, reading Delta tables from one storage container while the platform's own records are written to another" caption="Table data and the platform's own records are separate containers in one account. The Engine's own catalog is a file on the coordinator." />

    <h2>Health and readiness</h2>
    <Code lang="bash">{`curl -fsS https://<api-host>/api/health      # the API — note the /api prefix
curl -fsS http://<engine-host>/health        # liveness: the process answers
curl -fsS http://<engine-host>/ready         # readiness: it can serve`}</Code>
    <p>
      Liveness proves a process responds; readiness proves its dependencies — catalogs and the system store — are
      reachable. Monitor both separately, and alert on readiness rather than liveness: a coordinator that is up but
      cannot read its store will answer <code>/health</code> and fail every query.
    </p>
    <p>
      A worker appearing healthy says nothing about whether the cluster returns correct answers for a given topology.
      Check the Engine console&rsquo;s worker count against what you deployed before trusting a result.
    </p>

    <h2>Back up both places Kaveon keeps its memory</h2>
    <p>
      This is the part most easily got wrong, because only two of the three pieces of state are in object storage.
    </p>
    <table>
      <thead><tr><th>What</th><th>Where</th><th>How to back it up</th></tr></thead>
      <tbody>
        <tr>
          <td>The platform&rsquo;s records — datasets, charts, dashboards, saved statements, chat history, favourites, audit</td>
          <td>The system store: the configured container and prefix, or a directory</td>
          <td>The provider&rsquo;s versioning and retention; a directory copy for a local store</td>
        </tr>
        <tr>
          <td>The Engine&rsquo;s catalog — catalogs, schemas, every table definition, planner statistics, cubes</td>
          <td><code>/var/lib/kaveon/catalog.db</code> on the coordinator</td>
          <td>Copy it with the coordinator stopped, or <code>sqlite3 catalog.db &quot;.backup&quot;</code> while it runs</td>
        </tr>
        <tr>
          <td>Table data</td>
          <td>Wherever each catalog points</td>
          <td>Backed up where it lives; Kaveon only reads it</td>
        </tr>
      </tbody>
    </table>
    <Callout type="warn">
      <strong>A plain copy of a live SQLite database can be torn.</strong> It has a write-ahead log, so copy it with
      the coordinator stopped or use <code>.backup</code>. And verify a restore: a backup nobody has restored from is
      not yet a backup. A restore that brought the records back and left this catalog empty cost twenty-eight table
      registrations, rebuilt by hand.
    </Callout>

    <h2>Replacing a host</h2>
    <p>
      Losing the machine is not losing the data. Table data and the system store are untouched when both are in
      object storage. What does not come back on its own is the Engine&rsquo;s catalog, so a new host comes up able to
      read everything and knowing about nothing.
    </p>
    <p>
      With <code>catalog.db</code> restored from backup, the stack comes up as it was. Without it, rebuild the map:
    </p>
    <Code lang="bash">{`# 1. Bring the stack up against the same system store, so the dashboards,
#    charts and datasets are already there.
./scripts/kaveon-up.sh --storage adls://<account>/<container>/<prefix>

# 2. Re-register each catalog, its schemas and its tables. Columns are read
#    from each table's own metadata; re-running skips what already exists.
python scripts/register-lake-catalog.py --api http://localhost:8082 \\
    --catalog OpenSource --account <account> --container <container> \\
    --root snapshots/<version> \\
    --schema public:<table>,<table>

# 3. Re-measure. ANALYZE each table, from SQL Lab or the Catalog page.`}</Code>
    <p>
      Expect step 3 to take a while, and expect dashboards to be <em>right and slow</em> until it finishes: a tile that
      was answered from a cube goes back to scanning its table until that table&rsquo;s cube exists again. The cube
      pass gives each worker a single task over all of its files under a hard 600&nbsp;second ceiling, so a large table
      may need more workers to fit inside it — bring them up to build, then take them down.
    </p>

    <h2>Routine checklist</h2>
    <ul>
      <li>After every deploy, verify a Studio-to-API request and each sign-in provider&rsquo;s callback.</li>
      <li>Test registered sources and a representative read-only query per source.</li>
      <li>Track query latency and error rate, DLM freshness, and context rebuild failures.</li>
      <li>Watch the system store&rsquo;s size and the coordinator&rsquo;s disk: cubes, statistics and the exchange spool all live on the host.</li>
      <li>Rotate provider secrets, <code>KAVEON_PROXY_SECRET</code> and the Engine bridge and catalog tokens through managed secret storage. The proxy secret must change on both tiers together.</li>
      <li>Keep a copy of the environment file. It holds the only record of which provider, which admins and which storage a deployment was given.</li>
    </ul>

    <h2>Troubleshooting order</h2>
    <ol>
      <li>Identify the failing boundary: browser, Studio proxy, API, the Engine, the system store, or a registered source.</li>
      <li>Correlate the status code, request id, deployment revision and server logs — without recording credentials.</li>
      <li>Check configuration and network reachability before changing data or restarting anything.</li>
      <li>Reproduce with the smallest read-only request, and compare <code>/health</code> against <code>/ready</code>.</li>
      <li>For a slow answer rather than a failed one, read the lane the Engine reported: a tile that used to come from a cube and now scans means its table&rsquo;s statistics or cube are missing, not that the query is wrong.</li>
    </ol>
    <Callout type="note">
      <code>STATUS.md</code> is the capability ledger, <code>SECURITY.md</code> covers trust boundaries, and{" "}
      <code>DEPLOYMENT.md</code> carries the current topology. <a href="/docs/deployment">Deployment</a> has the
      storage layout and the environment variables.
    </Callout>
    <Pager prev={{ href: "/docs/auth", title: "Auth & RBAC" }} next={{ href: "/docs/troubleshooting", title: "Troubleshooting" }} />
  </div>;
}
