import { Callout, Code, PageHeader, Pager } from "../../../../components/docs/prose";

export const metadata = { title: "Engine Architecture" };
export default function EngineArchitectureDocs() { return <div className="docs-prose">
  <PageHeader eyebrow="Engine Manual" title="Architecture and startup" lead="The coordinator plans and schedules; workers execute validated vectorized fragments and exchange Arrow partitions." />
  <Code lang="text">{`Client → coordinator: catalog → SQL → optimizer → stage DAG
Coordinator → workers: authenticated executable fragments
Workers → workers: bounded Arrow IPC exchange
Workers → coordinator: measured root results and task telemetry`}</Code>
  <h2>Runtime responsibilities</h2><p>The coordinator owns catalog resolution, planning, stage construction, assignment, retry, cancellation, and root-result collection. Workers advertise routable endpoints, validate fragment v2, scan deterministic partitions, run physical operators, and publish exchange outputs.</p>
  <h2>Crates</h2><p><code>core</code> defines contracts; <code>storage</code> reads lake data; <code>exec</code> runs operators; <code>sql</code> and <code>optim</code> plan; <code>catalog</code> persists metadata; <code>server</code> coordinates workers; <code>cli</code> is the remote client.</p>
  <Callout type="warn">Studio and FastAPI do not yet route analytical queries through Engine. Root results are synchronous, metadata is single-coordinator, and end-user Engine auth/TLS is not production-qualified.</Callout>
  <Pager prev={{ href: "/docs/engine", title: "Kaveon Engine" }} next={{ href: "/docs/engine/sql", title: "Engine SQL" }} />
</div>; }
