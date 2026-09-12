import { Callout, PageHeader, Pager } from "../../../../components/docs/prose";

export const metadata = { title: "KaveonDB scorecard" };

const analytics: [string, number, string][] = [
  ["Throughput, matched corpus", 7, "2.83 QPS vs Trino 2.24 on the twelve-query corpus at matched resources; 66.5% of the 1.90× objective"],
  ["Scale at 504M rows", 5, "Exact grouped aggregate in ~20 s on three 3-CPU workers; wide aggregates 3–10× slower; exact distinct fix pending qualification"],
  ["SQL surface", 6, "CTEs, HAVING, CASE, window functions, WHERE subqueries, set operations; correlated subqueries, GROUPING SETS and approximate aggregates absent"],
  ["Storage and formats", 5, "Parquet, Delta (no checkpoints), Iceberg readers on ADLS Gen2 and local disk; no S3, no table writes"],
  ["Federation", 1, "No connectors; the Engine reads lake files. Out of scope by design"],
  ["Distributed execution", 6, "Retry to an alternate worker, cancellation, restart reconciliation verified on AKS; aggregate and join spill still open"],
  ["Memory and admission", 5, "Bounded by design; a 504M-row table found a leak on one digest, closed on the next"],
  ["Optimizer", 4, "Pushdown, pruning, exact-statistics broadcast; no cost-based reordering or dynamic filtering"],
  ["Clients and ecosystem", 2, "HTTP statement API and CLI only; no JDBC/ODBC or wire compatibility"],
  ["Security", 6, "Entra identity, TLS, owner-bound records, revision conflicts; no row filters or column masks"],
  ["Operability", 5, "Reproducible images, Helm, console, telemetry; no backup/restore or upgrade qualification"],
];

const transactions: [string, number, string][] = [
  ["ACID for product records", 6, "CAS head publication, snapshot-bound reads, conflict proofs; no cross-kind transactions"],
  ["General relational DML", 1, "Record kinds only, by design"],
  ["Constraints and indexes", 2, "Key uniqueness and reference validation only"],
  ["Isolation and concurrency", 4, "Snapshot reads and revision conflicts; one statement per request"],
  ["Durability and recovery", 4, "Object-storage durability; head recovery and restore not yet qualified"],
  ["Query over records", 5, "Bounded typed reads through the Engine"],
  ["OLTP-shaped latency", 3, "A publication is an ADLS conditional write"],
  ["Ecosystem", 1, "No wire protocol or drivers"],
  ["Operational maturity", 3, "Retained snapshots and a fail-closed migration; PostgreSQL remains authoritative"],
];

function Scores({ rows }: { rows: [string, number, string][] }) {
  return (
    <table>
      <thead><tr><th>Dimension</th><th>Score</th><th>Evidence and gap</th></tr></thead>
      <tbody>
        {rows.map(([name, score, why]) => (
          <tr key={name}><td>{name}</td><td><strong>{score}</strong> / 10</td><td>{why}</td></tr>
        ))}
      </tbody>
    </table>
  );
}

export default function KaveonDbScorecardDocs() {
  return <div className="docs-prose">
    <PageHeader eyebrow="Research" title="KaveonDB scorecard" lead="Where KaveonDB stands against Trino for analytics and against PostgreSQL for the product's record store, rated per dimension from measured evidence." />
    <Callout type="note">Scores are relative to the reference system doing that job in production today, assessed September 12, 2026. Every measurement is recorded in the coordination log with a query ID. The two axes are not averaged: an average would hide the dimensions that decide whether the product ships.</Callout>

    <h2>Analytics — against Trino 483</h2>
    <Scores rows={analytics} />
    <p><strong>Standing.</strong> On the workload it was built for, KaveonDB completes more exact queries per second than Trino on matched resources, with exact statistics and fail-closed semantics Trino does not have. Outside that workload — federation, SQL breadth, ecosystem, optimizer — Trino is years ahead.</p>

    <h2>Transactions — against PostgreSQL 18</h2>
    <Scores rows={transactions} />
    <p><strong>Standing.</strong> KaveonDB is a typed product-record protocol on object storage, not a relational database, and does not claim to be one. Recovery, isolation and latency must reach the middle of the scale before PostgreSQL can be retired.</p>

    <h2>What may be claimed</h2>
    <ul>
      <li>26% more exact queries per second than Trino 483 on the declared corpus at matched resources — with the workload stated.</li>
      <li>An exact grouped aggregate over 504 million rows in about 20 seconds on three small workers — not &ldquo;interactive.&rdquo;</li>
      <li>Not 1.9× Trino, not a category of one, not PostgreSQL-free — yet.</li>
    </ul>

    <h2>Canonical paper</h2>
    <p>The full assessment, per-dimension evidence, and the five changes that move the scores live in <a href="https://github.com/PruthviProdduturi/Kaveon/blob/dev/docs/research/kaveondb-scorecard-trino-postgresql.md">Where KaveonDB stands</a>.</p>
    <Pager prev={{ href: "/docs/research/htap-platforms", title: "Kaveon vs HTAP platforms" }} />
  </div>;
}
