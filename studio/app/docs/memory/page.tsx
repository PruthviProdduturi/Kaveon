import { Callout, PageHeader, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Engine Memory" };

export default function EngineMemoryDocs() {
  return <div className="docs-prose">
    <PageHeader eyebrow="Build & Operate" title="Engine memory management" lead="Kaveon reserves retained execution state against explicit query budgets and fails closed when a bounded operator cannot continue safely." />
    <Callout type="warn"><strong>Alpha boundary:</strong> admission and hash aggregate/join accounting exist as opt-in execution contracts, but coordinator and worker planning do not yet propagate them universally. Aggregate and join spill remain release gates.</Callout>

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
      <tr><td>Hash aggregate</td><td>Accounts group and exact-distinct state; rejects growth beyond the query limit.</td></tr>
      <tr><td>Hash join</td><td>Accounts retained inputs, build index, match bitmap, and output-index growth; rejects growth beyond the query limit.</td></tr>
      <tr><td>Exchange</td><td>Independent byte and exchange-count ceilings with cleanup accounting.</td></tr>
    </tbody></table>

    <h2>What remains</h2>
    <p>Production readiness requires universal planner wiring, queued admission, partitioned aggregate and join spill, measured operator telemetry, and stress evidence for skew, concurrency, cancellation, retry, and worker loss. Until then, deployments must not claim global memory enforcement.</p>
    <Pager prev={{ href: "/docs/operations", title: "Operations" }} next={{ href: "/docs/sql-compatibility", title: "SQL Compatibility" }} />
  </div>;
}
