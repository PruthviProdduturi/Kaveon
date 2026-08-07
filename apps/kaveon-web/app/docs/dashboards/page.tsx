import { PageHeader, Callout, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Dashboards" };

export default function DashboardsDocs() {
  return (
    <div className="docs-prose">
      <PageHeader
        eyebrow="Features"
        title="Dashboards"
        lead="Dashboards arrange charts on a drag-and-drop canvas with cross-filtering, dashboard-level filters, auto-refresh, and dark mode. Save as a draft or publish to your whole team."
      />

      <h2>Creating a dashboard</h2>
      <ol>
        <li>Go to <strong>Dashboards → + New Dashboard</strong> — an empty canvas opens in edit mode.</li>
        <li>Add charts and text blocks from the sidebar.</li>
        <li>Drag and resize tiles into place.</li>
        <li><strong>Save</strong> as a draft, or <strong>Publish</strong> to make it visible to everyone.</li>
      </ol>

      <h2>Edit mode vs view mode</h2>
      <p>
        The builder enters edit mode when you create or edit a dashboard. Published dashboards open in view mode for
        non-owners; owners get an <strong>Edit</strong> button to return.
      </p>
      <table>
        <thead><tr><th>Capability</th><th>Edit</th><th>View</th></tr></thead>
        <tbody>
          <tr><td>Drag / resize / add / remove tiles</td><td>Yes</td><td>No</td></tr>
          <tr><td>Cross-filter clicks</td><td>Yes</td><td>Yes</td></tr>
          <tr><td>Auto-refresh</td><td>Yes</td><td>Yes</td></tr>
          <tr><td>Chart interactions (zoom, tooltip)</td><td>Yes</td><td>Yes</td></tr>
        </tbody>
      </table>

      <h2>Canvas layout</h2>
      <p>
        The canvas uses <a href="https://github.com/react-grid-layout/react-grid-layout" target="_blank" rel="noreferrer">react-grid-layout v2</a>.
        The grid is <strong>12 columns</strong> on desktop and tablet (12 lg/md), reflowing to 6 (small), 2 (mobile),
        and 1 (extra-small); the default tile is 6×8. Vertical
        compaction pulls tiles up to fill gaps, and the layout reflows responsively at each breakpoint. Every tile stores
        its <code>x, y, w, h</code> in the dashboard JSON.
      </p>

      <h2>Adding content</h2>
      <p>
        In edit mode, <strong>Add Chart</strong> opens a picker of all saved charts — search by name, filter by owner
        (Mine / All), and select several at once; they drop in as new tiles. <strong>Add Text</strong> inserts a markdown
        block for section headers, annotations, or narrative.
      </p>

      <h2>Filters &amp; cross-filtering</h2>
      <p>
        The <strong>Filters</strong> tab adds dashboard-level filters that render as a bar above the canvas; a filter
        applies to every chart sharing that column, so one chip narrows the whole board at once.
      </p>
      <Callout type="tip">
        <strong>Cross-filtering:</strong> click a bar, slice, or map marker and Kaveon sets a filter on that value — every
        other chart querying the same column updates in real time. It&rsquo;s additive: click values across charts and the
        filters stack.
      </Callout>

      <h2>Publishing &amp; auto-refresh</h2>
      <p>
        Dashboards start as drafts (visible only to you). <strong>Publish</strong> makes them visible to all authenticated
        users with the right role; set the status back to draft to unpublish.
      </p>
      <p>
        Each tile can auto-refresh on an interval — <strong>Off, 30s, 1m, 5m, 10m, or 30m</strong> — re-running its SQL
        against the live source and updating in place, no full reload. Auto-refresh pauses in edit mode so it doesn&rsquo;t
        disrupt dragging.
      </p>

      <Pager
        prev={{ href: "/docs/charts", title: "Chart Builder" }}
        next={{ href: "/docs/datasets", title: "Semantic Datasets" }}
      />
    </div>
  );
}
