import { Code, PageHeader, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Troubleshooting" };

export default function TroubleshootingDocs() {
  return <div className="docs-prose">
    <PageHeader eyebrow="Deploy &amp; Operate" title="Troubleshooting" lead="Start at the runtime boundary that failed, capture sanitized evidence, and account for process-local state." />
    <h2>Health checks</h2>
    <Code lang="text">{`FastAPI: GET /api/health
Engine:  GET /health
Engine:  GET /ready   # requires a loaded catalog`}</Code>
    <h2>Authentication and proxy</h2>
    <p>Verify <code>API_URL</code>, matching <code>KAVEON_PROXY_SECRET</code> values, <code>AUTH_URL</code>, and provider callback URLs. Set <code>WEB_URL</code> to the exact Studio origin. Never expose the proxy secret to browser code.</p>
    <h2>Storage and catalog connectivity</h2>
    <p>Check the registered catalog location, credential reference, source version, TLS, and object-store permissions. A table is activated only after a metadata probe; run <code>SHOW TABLES</code>, <code>DESCRIBE</code>, or a bounded <code>SELECT</code> in SQL Lab to distinguish catalog, storage, and query failures.</p>
    <h2>Known qualification boundaries</h2>
    <ul><li>Workers execute versioned distributed fragments with admission control, bounded aggregate/join spill, retry, and cancellation. Cloud-scale skew, disk exhaustion, and sustained multi-node endurance remain qualification gates.</li><li>Terminal Engine query history is journaled and restored on restart; paged result pages and process-local result/scan caches expire by design and are not durable product records.</li><li>Query cancellation propagates to active distributed work, but synchronous operators observe cancellation at their documented execution boundaries.</li><li>Parquet, Delta, and Iceberg are qualified locally; declared ADLS Gen2 datasets have local/native evidence, while S3 and broader cloud deployment qualification remain staged.</li></ul>
    <h2>Escalation</h2>
    <p>Record commit, component, timestamp/time zone, route or sanitized SQL, database type, topology, and restart behavior. Never publish tokens, <code>.env</code>, connection strings, or customer rows.</p>
    <Pager prev={{ href: "/docs/operations", title: "Operations" }} next={{ href: "/docs/upgrades", title: "Upgrades" }} />
  </div>;
}
