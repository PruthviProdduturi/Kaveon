import { PageHeader, Callout, Code, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Freshness" };

export default function FreshnessDocs() {
  return (
    <div className="docs-prose">
      <PageHeader
        eyebrow="Intelligence"
        title="Freshness"
        lead="Kaveon answers a great deal without reading the data, which only works if it can tell when a precomputed answer has stopped being true. Deciding that is itself a metadata read, never a scan."
      />

      <p>
        Two layers precompute, and they decide freshness differently. The Engine&rsquo;s statistics and cube cells are
        tied to an exact version of a table and are discarded the moment it moves. The DLM&rsquo;s context artifact
        carries a score, but what invalidates it is a measured change, not age.
      </p>
      <table>
        <thead><tr><th>Layer</th><th>What it precomputes</th><th>Freshness rule</th></tr></thead>
        <tbody>
          <tr>
            <td>The Engine</td>
            <td>Table statistics, cube cells, HyperLogLog sketches</td>
            <td>Exact: the recorded source version either is the table&rsquo;s current one or it is not</td>
          </tr>
          <tr>
            <td>The DLM</td>
            <td>A dataset&rsquo;s context artifact — totals, per-dimension breakdowns, low-cardinality combinations</td>
            <td>Scored, but only a detected change makes it stale</td>
          </tr>
        </tbody>
      </table>

      <h2>The Engine: source version identity</h2>
      <p>
        Every statistics record and every cube carries the <code>SourceVersion</code> it was computed over — a digest
        of the table&rsquo;s location together with its version identity, which is the Delta log version, the Iceberg
        snapshot id, or the file listing for a directory of Parquet.
      </p>
      <Code lang="rust">{`pub struct SourceVersion {
    /// Location plus the Delta version, Iceberg snapshot, object version
    /// or listing digest, hashed together.
    pub identity_sha256: String,
    pub kind: SourceVersionKind,   // DeltaVersion | IcebergSnapshot | Listing
}

impl TableCube {
    pub fn is_current_for(&self, identity_sha256: &str) -> bool {
        self.source_version.identity_sha256 == identity_sha256
    }
}`}</Code>
      <p>
        There is no score and no tolerance here. A cube built over version 41 is not used to answer a question about
        version 42; the planner falls back to a distributed scan and the cube waits for the next{" "}
        <code>ANALYZE</code>. A changed shape declaration rebuilds for the same reason. This is why a tile that used
        to return in under a second can suddenly take much longer after a table is rewritten: the answer is still
        correct, it is just no longer precomputed. See <a href="/docs/engine">Kaveon Engine</a> for what makes a
        question cube-shaped in the first place.
      </p>
      <Callout type="note">
        Checking this costs a metadata read, not a scan — the Delta log tail, the snapshot pointer, or a listing. That
        is the whole idea: <strong>detecting that data changed never requires reading the data</strong>.
      </Callout>

      <h2>The DLM: a score, and what actually invalidates it</h2>
      <p>
        A dataset&rsquo;s context artifact gets a score from two factors, and the score is reported — but the decision
        does not rest on it alone:
      </p>
      <Code lang="python">{`score = time_factor(age) * change_factor(rows_changed)

fresh = (not data_modified) or score >= 0.5`}</Code>
      <Callout type="warn">
        <strong>Age alone never invalidates context.</strong> A table nobody has written to is answered from context
        however old the artifact is. This is deliberate: rebuilding on a timer is a full scan for nothing, and on a
        small host it is a full scan that may not finish. Expect to see a score of <code>0.0</code> alongside{" "}
        <code>&quot;fresh&quot;: true</code> — that is an artifact built weeks ago over data that has not moved, and
        it is the correct answer.
      </Callout>
      <p>
        The change signal depends on where the dataset lives, and this is the part worth understanding:
      </p>
      <ul>
        <li>
          <strong>An Engine-backed dataset</strong> compares the source version its artifact recorded against the
          table&rsquo;s current one. The digests match or they do not, so the change is a fact rather than an
          estimate. A moved version is scored as at least the half fraction, which puts any artifact with age past
          the threshold — in practice, <em>a version that moved means rebuild</em>.
        </li>
        <li>
          <strong>A registered external database</strong> has no such identity, so drift is inferred from the
          database&rsquo;s own modification counter. This path needs a source that exposes those counters and is not
          available in a deployment without one.
        </li>
      </ul>
      <Callout type="warn">
        <strong>A dataset whose artifact carries no Engine binding detects no change at all.</strong> Its{" "}
        <code>data_modified</code> stays false, so it reports <code>use_context</code> forever and is never rebuilt
        automatically — even if the table underneath is replaced. An artifact compiled before its dataset was bound
        to an Engine table is in exactly this position, and the only fix is to recompile it so the binding is
        recorded. Check the <code>signal</code> field to tell which kind you have.
      </Callout>

      <h2>The numbers</h2>
      <table>
        <thead><tr><th>Constant</th><th>Value</th><th>What it does</th></tr></thead>
        <tbody>
          <tr><td><code>BASE_HALF_LIFE_SECONDS</code></td><td>6 hours</td><td>The time factor halves every 6 hours. It lowers confidence in the score; it does not by itself make context stale.</td></tr>
          <tr><td><code>CHANGE_HALF_FRACTION</code></td><td>0.05</td><td>Five percent of rows changed halves the change factor.</td></tr>
          <tr><td>Staleness threshold</td><td>0.5</td><td>With a detected change, the score must reach this to keep using context.</td></tr>
        </tbody>
      </table>
      <p>
        Usage weighting exists in the scorer — frequently relied-upon elements were meant to decay faster, shortening
        the effective half-life — but dataset freshness does not pass a usage count, so it is not in effect on this
        path. It is described here because the code is there, not because it changes an answer.
      </p>

      <h2>What rebuilds a stale artifact</h2>
      <table>
        <thead><tr><th>Trigger</th><th>Status</th></tr></thead>
        <tbody>
          <tr>
            <td><strong>On ask.</strong> A question that routes through the DLM checks freshness after serving, and starts a background rebuild if the artifact is stale. The asker gets their answer immediately; the next asker gets the rebuilt context.</td>
            <td>Active</td>
          </tr>
          <tr>
            <td><strong>Pipeline notification.</strong> <code>POST /dlm/notify-data-change?dataset_id=…</code> clears the in-memory context and starts a rebuild, so a load that has just finished does not wait to be noticed.</td>
            <td>Active</td>
          </tr>
          <tr>
            <td><strong>Manual sweep.</strong> <code>POST /dlm/sweep</code> checks every compiled artifact the caller can read and rebuilds the stale ones. Administrator only.</td>
            <td>Active</td>
          </tr>
          <tr>
            <td><strong>The automatic background sweep.</strong> A daemon thread was intended to run the same check every 30 minutes.</td>
            <td><strong>Not running</strong></td>
          </tr>
        </tbody>
      </table>
      <Callout type="warn">
        <strong>Nothing rebuilds a dataset nobody asks about.</strong> The periodic sweep does not start, because
        reading compiled artifacts requires a caller identity and an unattended thread has none — it reports that
        rather than silently checking nothing. So a dataset that is loaded but never queried keeps a stale artifact
        until someone asks, a pipeline notifies, or an administrator sweeps. If your data arrives on a schedule, call
        the notification endpoint from the pipeline; that is the supported path, not a timer.
      </Callout>

      <h2>Reading a freshness report</h2>
      <Code lang="json">{`GET /datasets/24/freshness

{
  "fresh": true,
  "score": 0.0,
  "computed_at": "2026-09-12T08:35:02.373996",
  "data_modified": false,
  "recommendation": "use_context",
  "signal": "engine_source_version",
  "source_version":         { "identity_sha256": "…", "kind": "delta_version", "version": 41 },
  "current_source_version": { "identity_sha256": "…", "kind": "delta_version", "version": 41 },
  "observed_at_ms": 1760042435000
}`}</Code>
      <table>
        <thead><tr><th>Field</th><th>How to read it</th></tr></thead>
        <tbody>
          <tr><td><code>recommendation</code></td><td><code>use_context</code>, <code>rebuild</code>, or <code>no_context</code> when there is no ready artifact. This is the decision; the score is evidence for it.</td></tr>
          <tr><td><code>data_modified</code></td><td>Whether a change was actually detected. False with a low score means old but unchanged, which is fresh.</td></tr>
          <tr><td><code>signal</code></td><td>Which change signal was used. <code>engine_source_version</code> is the exact one. Anything else means the artifact has no Engine binding — see the warning above.</td></tr>
          <tr><td><code>source_version</code> vs <code>current_source_version</code></td><td>What the artifact was built over, and what the table is now. Equal digests are the reason a precomputed answer is allowed to stand.</td></tr>
        </tbody>
      </table>
      <p>
        Studio reports the same distinction per answer, so a reader never has to guess whether a number came from
        precomputed context or from a query that has just run.
      </p>

      <Pager prev={{ href: "/docs/nl-to-sql", title: "DLM · NL→SQL" }} next={{ href: "/docs/engine", title: "Kaveon Engine" }} />
    </div>
  );
}
