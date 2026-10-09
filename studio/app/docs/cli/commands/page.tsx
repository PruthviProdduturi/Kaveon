import { Code, PageHeader, Pager } from "../../../../components/docs/prose";

export const metadata = { title: "Interactive commands — Kaveon CLI" };

export default function CliCommandsDocs() {
  return <div className="docs-prose">
    <PageHeader eyebrow="CLI" title="Interactive commands" lead="Browse the session and metadata without sending shell commands to the SQL parser." />
    <h2>Context and metadata</h2>
    <Code lang="text">{`.catalogs                 list granted catalogs
.schemas OpenSource       list schemas in a catalog
.tables                   list tables in the active schema
.use OpenSource.public    switch catalog and schema
.settings                 show session settings
.queries                  inspect recent queries`}</Code>
    <h2>Control work</h2>
    <Code lang="text">{`.kill <query-id>          cancel a running query
.watch <query-id>         follow state and counters
.explain                  show the last execution plan
.help                     show command help
.exit                     leave the shell`}</Code>
    <p>Use <code>Ctrl-C</code> once to cancel the running statement and again to return to the editor. The prompt shows the active catalog and schema; output includes elapsed time, rows, columns, workers, and the query id.</p>
    <h2>Shell behaviour</h2>
    <ul>
      <li>Persistent history, reverse search, tab completion, and Emacs or Vi editing.</li>
      <li>Paged results arrive while a streaming query runs; <code>Space</code> or <code>Enter</code> advances pages and <code>q</code> stops paging.</li>
      <li>Dot commands stay client-side; SQL statements are sent to the Engine.</li>
    </ul>
    <Pager prev={{ href: "/docs/cli/install", title: "Install & connect" }} next={{ href: "/docs/cli/sql", title: "SQL & catalog administration" }} />
  </div>;
}
