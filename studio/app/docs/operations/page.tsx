import { Callout, Code, PageHeader, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Operations" };

export default function OperationsDocs() {
  return <div className="docs-prose">
    <PageHeader eyebrow="Deploy &amp; Operate" title="Operations" lead="Verifying a Kaveon deployment, protecting its trust boundaries, backing up the two places it keeps its memory, and rebuilding the catalog when a host is replaced." />

    <h2>Know your deployment</h2>
    <p>
      Start at the configured Studio origin. Studio forwards authenticated requests to the API. Eligible DLM questions use compiled context. Live lake queries run through the coordinator and workers. See <a href="/docs/deployment">Deployment topology</a> for service placement.
    </p>

    <h2>Health and readiness</h2>
    <p>On the Compose host, check the API, coordinator process and coordinator readiness separately. On AKS, use private service endpoints and the deployment’s TLS configuration.</p>
    <Code lang="bash">{`curl -fsS http://localhost:8082/api/health
curl -fsS http://localhost:8081/health
curl -fsS http://localhost:8081/ready`}</Code>
    <p>Engine readiness checks for a loaded catalog and, on workers, synchronization with the coordinator’s catalog snapshot. Verify storage credentials separately by reading a registered table in SQL Lab.</p>
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
      Restore the coordinator catalog and reconnect to the original product-store location. Preserve any local volumes that hold product or source data. If registrations are missing, use your catalog archive or register the catalogs and tables again. Verify each table with a bounded read, refresh its statistics and rebuild any required cubes before comparing dashboard performance.
    </p>
    <p>For an ADLS catalog, the registration utility accepts the account, container, prefix and schema-to-table mappings. Inspect the inputs before running it against the restored installation.</p>
    <Code lang="bash">python scripts/register-lake-catalog.py --help</Code>

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
