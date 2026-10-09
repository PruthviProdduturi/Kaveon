import { Code, PageHeader, Pager } from "../../../../components/docs/prose";

export const metadata = { title: "Automation and output — Kaveon CLI" };

export default function CliAutomationDocs() {
  return <div className="docs-prose">
    <PageHeader eyebrow="CLI" title="Automation and output" lead="Use the same client in CI, shell scripts, and reproducible data checks." />
    <h2>Run statements non-interactively</h2>
    <Code lang="bash">{`kaveon --local --data-dir ./warehouse -e "SELECT count(*) FROM orders"
kaveon --server https://engine.example.com -f checks.sql
cat checks.sql | kaveon --server https://engine.example.com --format JSONL`}</Code>
    <h2>Deterministic formats</h2>
    <Code lang="text">{`--format table     aligned terminal output
--format vertical  one row per block
--format csv       comma-separated values
--format tsv       tab-separated values
--format json      one JSON document
--format jsonl     one JSON object per row
--no-header        omit column names
--row-limit 5000   bound returned rows`}</Code>
    <p><code>--ignore-errors</code> continues a batch while preserving a failing exit status. Use <code>--pager never</code> in CI and set <code>NO_COLOR=1</code> for stable logs.</p>
    <h2>Inspect performance</h2>
    <p><code>EXPLAIN ANALYZE</code> reports the plan, stages, task counters, memory, exchange bytes, rows scanned, spill, and the lane that answered the statement. Keep query ids with your test artifacts.</p>
    <Pager prev={{ href: "/docs/cli/sql", title: "SQL & catalog administration" }} next={{ href: "/docs/engine", title: "Kaveon Engine" }} />
  </div>;
}
