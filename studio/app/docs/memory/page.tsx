import { Callout, PageHeader, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Engine Memory" };

export default function EngineMemoryDocs() {
  return <div className="docs-prose">
    <PageHeader eyebrow="Engine" title="Engine memory management" lead="Kaveon reserves retained execution state against explicit query budgets and fails closed when a bounded operator cannot continue safely." />
    <Callout type="warn"><strong>Alpha boundary:</strong> coordinator and worker execution propagate query budgets into retained operator state. Logical reservations are not a universal process RSS ceiling: decoders, runtime overhead, and retained caller buffers require separate limits and measurement.</Callout>

    <h2>Budget hierarchy</h2>
    <ol>
      <li>The admission controller reserves a complete query budget before execution.</li>
      <li>The query pool atomically enforces one hard limit across its operators.</li>
      <li>Named operator accounts expose current and peak reserved bytes.</li>
      <li>RAII reservations return capacity on success, error, cancellation, or destruction.</li>
    </ol>

    <h2>Operator behavior</h2>
    <table><thead><tr><th>Operator</th><th>Current bounded behavior</th></tr></thead><tbody>
      <tr><td>Sort / TopN</td><td>Opt-in reservations, bounded Arrow IPC spill runs, and fixed-fan-in merge.</td></tr>
      <tr><td>Hash aggregate</td><td>Accounts typed group/distinct state; opt-in partitioned Single/Partial/Final spill. Unsplittable skew fails closed.</td></tr>
      <tr><td>Hash join</td><td>Accounts retained inputs, build index, match bitmap, and output growth; opt-in partitioned spill.</td></tr>
      <tr><td>Window / set operations</td><td>Accounts buffered state and expression workspaces; cancellation checks interrupt long loops.</td></tr>
      <tr><td>Exchange</td><td>Bounded disk-backed coordinator exchange storage, query quotas, download leases, and cleanup accounting.</td></tr>
    </tbody></table>

    <h2>What remains</h2>
    <p>Pressure and worker-loss fixtures provide local evidence, including fail-closed skew and disk-quota cases. High-cardinality distributed aggregation, full pipeline backpressure, sustained soak tests, and cloud fault testing remain qualification gates. Do not equate a configured query budget with a hard process RSS limit.</p>
    <Pager prev={{ href: "/docs/engine/storage", title: "Storage & Catalogs" }} next={{ href: "/docs/sql-compatibility", title: "SQL Compatibility" }} />
  </div>;
}
