import { Code, PageHeader, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Troubleshooting" };

export default function TroubleshootingDocs() {
  return <div className="docs-prose">
    <PageHeader eyebrow="Operations" title="Troubleshooting" lead="Start at the runtime boundary that failed, capture sanitized evidence, and account for process-local state." />
    <h2>Health checks</h2>
    <Code lang="text">{`FastAPI: GET /api/health
Engine:  GET /health
Engine:  GET /ready   # requires a loaded catalog`}</Code>
    <h2>Authentication and proxy</h2>
    <p>Verify <code>API_URL</code>, matching <code>KAVEON_PROXY_SECRET</code> values, <code>AUTH_URL</code>, and provider callback URLs. Set <code>WEB_URL</code> to the exact Studio origin. Never expose the proxy secret to browser code.</p>
    <h2>Database connectivity</h2>
    <p>Check the <code>METADATA_*</code> settings, DNS/firewall access, TLS, driver installation, and database role. A registered source’s test endpoint is a stub; run <code>SELECT 1</code> in SQL Lab or use a setup/admin probe.</p>
    <h2>Expected alpha behavior</h2>
    <ul><li>Workers execute versioned distributed fragments, but admission control and aggregate/join spill are not yet implemented.</li><li>Engine query history and FastAPI job/cache state are process-local and disappear after restart.</li><li>Query cancellation propagates to active distributed work, but synchronous operators only observe cancellation at their current execution boundaries.</li><li>ADLS Gen2, S3, Iceberg, and external catalog adapters are contracts rather than executable data paths.</li></ul>
    <h2>Escalation</h2>
    <p>Record commit, component, timestamp/time zone, route or sanitized SQL, database type, topology, and restart behavior. Never publish tokens, <code>.env</code>, connection strings, or customer rows.</p>
    <Pager prev={{ href: "/docs/connectors", title: "Connector Matrix" }} next={{ href: "/docs/upgrades", title: "Upgrades" }} />
  </div>;
}
