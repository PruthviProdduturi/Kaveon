import { PageHeader, Callout, Code, Pager } from "../../../components/docs/prose";

export const metadata = { title: "SQL Lab" };

export default function SqlLabDocs() {
  return (
    <div className="docs-prose">
      <PageHeader
        eyebrow="Features"
        title="SQL Lab"
        lead="A full query editor for exploring your data sources directly — Monaco-powered, multi-tab, with a live schema browser, cancellable execution, and inline AI. Open it from Lab in the sidebar."
      />

      <h2>Editor</h2>
      <p>
        The editor is <a href="https://microsoft.github.io/monaco-editor/" target="_blank" rel="noreferrer">Monaco</a> —
        the same editor that powers VS Code — so syntax highlighting, multiple cursors, and inline errors work exactly
        as you&rsquo;d expect.
      </p>

      <h3>Keyboard shortcuts</h3>
      <table>
        <thead><tr><th>Action</th><th>Shortcut</th></tr></thead>
        <tbody>
          <tr><td>Run query</td><td><code>Ctrl/Cmd + Enter</code></td></tr>
          <tr><td>Format SQL</td><td><code>Shift + Alt + F</code></td></tr>
          <tr><td>Comment line</td><td><code>Ctrl/Cmd + /</code></td></tr>
          <tr><td>Find</td><td><code>Ctrl/Cmd + F</code></td></tr>
          <tr><td>Go to line</td><td><code>Ctrl/Cmd + G</code></td></tr>
        </tbody>
      </table>

      <h3>Autocomplete &amp; the schema browser</h3>
      <p>
        Monaco autocompletes SQL keywords out of the box. Table and column names come from the{" "}
        <strong>schema browser</strong> on the left: select a database to load its tables, then expand a table to
        pull in its columns. Once loaded, those names feed autocomplete as you type.
      </p>

      <h2>Multi-tab sessions</h2>
      <p>
        Every query runs in its own tab. Open more with the <code>+</code> button in the tab bar; each tab keeps its
        own query text and results for the session.
      </p>
      <Callout type="warn">
        Tabs are <strong>not</strong> persisted across page reloads. Use <strong>Save Query</strong> to keep a query
        across sessions — saved queries live in your history with their source database attached.
      </Callout>

      <h2>Selecting a database</h2>
      <p>
        The database selector in the toolbar lists every active data source. Pick one before running a query and the
        schema browser updates to match. If nothing is selected, the editor falls back to the{" "}
        <strong>metadata database</strong> (where the demo datasets live) — so you can always explore the platform&rsquo;s
        own metadata without wiring up a source first.
      </p>

      <h2>Running queries</h2>
      <p>Kaveon exposes two execution paths, chosen automatically by the Run button.</p>

      <h3>Synchronous execution</h3>
      <p>
        <code>POST /api/v1/lab/execute</code> — for short queries expected to return within the HTTP timeout. It
        returns columns, rows, and a row count in one response.
      </p>

      <h3>Cancellable async execution</h3>
      <p>
        <code>POST /api/v1/lab/query</code> — used by the primary Run button. The backend runs the query in an executor
        thread and polls for client disconnect every 500&nbsp;ms. If you close the tab or navigate away, it sends a
        cancel signal to the database cursor (<code>cursor.cancel()</code>) and returns <code>204 No Content</code>.
      </p>
      <Callout type="note">
        This is why abandoned queries don&rsquo;t pile up on your warehouse — Kaveon actively cancels the cursor when the
        client goes away, instead of letting long-running work accumulate server-side.
      </Callout>

      <h3>Execution response</h3>
      <Code lang="json">{`{
  "success": true,
  "columns": ["order_id", "total", "region"],
  "rows": [
    [1001, 249.90, "West"],
    [1002, 88.00,  "East"]
  ],
  "row_count": 2,
  "duration_ms": 143
}`}</Code>

      <h2>Saving &amp; history</h2>
      <p>
        <strong>Save Query</strong> stores the SQL plus its source database so you can reopen it later. Every executed
        query is also written to <strong>Query History</strong> — a per-user audit trail you can search, re-run, or
        promote into a saved query. Both live under <code>/lab/queries</code>.
      </p>

      <h2>Inline AI</h2>
      <p>
        Press the <strong>✦ wand</strong> in the toolbar, describe what you want in plain English, and the generated
        SQL is injected straight into the active tab — ready to review and run. For the full natural-language flow
        (dataset detection, pattern matching, automatic charts) see{" "}
        <a href="/docs/nl-to-sql">AI · NL→SQL</a>.
      </p>

      <Pager
        prev={{ href: "/docs/quickstart", title: "Quickstart" }}
        next={{ href: "/docs/nl-to-sql", title: "AI · NL→SQL" }}
      />
    </div>
  );
}
