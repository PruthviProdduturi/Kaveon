import { Callout, Code, Diagram, PageHeader, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Operations" };

export default function OperationsDocs() {
  return <div className="docs-prose">
    <PageHeader eyebrow="Build & Operate" title="Operations" lead="A practical operating checklist for verifying Kaveon, protecting its trust boundaries, and diagnosing failures." />

    <h2>Know your deployment</h2>
    <p>The production application path is Browser → Vercel Studio → private FastAPI → metadata database and registered sources. Kaveon Engine is a separate alpha runtime until platform integration ships.</p>
    <Diagram src="/docs/architecture/kaveon-deployment-topology.svg" alt="Kaveon production and Engine alpha deployment boundaries" caption="Operate the shipping platform and alpha Engine as distinct security and failure domains until the target integration is implemented." />
    <Callout type="warn">Never infer production readiness from an Engine worker appearing healthy. Engine HTTP lacks authentication and TLS, and distributed execution is not yet implemented.</Callout>

    <h2>Health and readiness</h2>
    <Code lang="bash">{`curl -fsS https://<api-host>/health
curl -fsS https://<engine-host>/health
curl -fsS https://<engine-host>/ready`}</Code>
    <p>Liveness proves a process responds. Readiness should additionally prove required dependencies such as catalogs or metadata storage are available. Monitor both separately.</p>

    <h2>Routine checklist</h2>
    <ul>
      <li>Verify Studio-to-API proxy requests and authentication-provider callbacks after every deploy.</li>
      <li>Test registered sources, connection-pool recovery, and representative read-only queries.</li>
      <li>Track database capacity, query latency/error rate, DLM freshness, and context rebuild failures.</li>
      <li>Back up <code>kaveonmeta</code>, test restoration, and version schema migrations with releases.</li>
      <li>Rotate provider secrets and <code>KAVEON_PROXY_SECRET</code> through managed secret storage.</li>
    </ul>

    <h2>Troubleshooting order</h2>
    <ol>
      <li>Identify the failing boundary: browser, Studio proxy, FastAPI, metadata store, or registered source.</li>
      <li>Correlate status code, request ID, deployment revision, and server logs without recording credentials.</li>
      <li>Check configuration and network reachability before changing data or restarting services.</li>
      <li>Reproduce with the smallest read-only request and compare health versus readiness.</li>
    </ol>
    <Callout type="note">Use repository <code>STATUS.md</code> as the capability ledger, <code>SECURITY.md</code> for trust boundaries, and <code>DEPLOYMENT.md</code> for current topology.</Callout>
    <Pager prev={{ href: "/docs/deployment", title: "Deployment" }} next={{ href: "/docs/research", title: "Papers & Patents" }} />
  </div>;
}
