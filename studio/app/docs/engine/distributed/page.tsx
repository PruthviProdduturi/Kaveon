import { Callout, PageHeader, Pager } from "../../../../components/docs/prose";

export const metadata = { title: "Distributed Runtime" };
export default function DistributedRuntimeDocs() { return <div className="docs-prose">
  <PageHeader eyebrow="Engine Manual" title="Distributed runtime" lead="Validated stage DAGs, versioned fragments, deterministic partitioning, authenticated Arrow exchange, retry, and cancellation form the current distributed alpha." />
  <h2>Query shapes</h2><table><thead><tr><th>Shape</th><th>Evidence</th></tr></thead><tbody>
    <tr><td>Scan/filter/project</td><td>Distributed and Docker-verified locally</td></tr><tr><td>Aggregate</td><td>Partial/final weighted AVG and exact distinct transport</td></tr><tr><td>Sort/TopN</td><td>Partial/final execution with bounded spill merge</td></tr><tr><td>Equi-join</td><td>Hash repartition; representative inner join Docker verification</td></tr><tr><td>Cross/outer joins</td><td>Contracts/local semantics; broader distributed evidence pending</td></tr>
  </tbody></table>
  <h2>Exchange and recovery</h2><p>Arrow IPC v2 identifies query, stage, exchange, attempt, and partition. Authenticated stores enforce byte/count ceilings and idempotency. Retry rotates attempts, stale work fails closed, cancellation propagates, and cleanup releases state.</p>
  <Callout type="warn">Streaming exchange, durable spooling, skew mitigation, cost-based distribution, admission/resource groups, and sustained worker-loss/concurrency evidence remain production gates.</Callout>
  <Pager prev={{ href: "/docs/engine/sql", title: "Engine SQL" }} next={{ href: "/docs/engine/storage", title: "Storage & Catalogs" }} />
</div>; }
