import { Callout, PageHeader, Pager } from "../../../../components/docs/prose";

export const metadata = { title: "Engine SQL" };
export default function EngineSqlDocs() { return <div className="docs-prose">
  <PageHeader eyebrow="Engine Manual" title="SQL and execution" lead="A feature is complete only when parsing, semantics, physical execution, distributed fragments, tests, and documentation agree." />
  <h2>Committed baseline</h2><p>Scans, filters, projections, arithmetic/comparisons, aliases, grouped/global aggregates, exact <code>COUNT(DISTINCT ...)</code>, ordering, limits, and inner/left/right/full/cross joins execute on committed <code>dev</code>.</p>
  <h2>Active integration</h2><p>CASE, LIKE/ILIKE, BETWEEN, IN, CAST, concatenation, OFFSET, row DISTINCT, UNION ALL, HAVING, CTEs, derived tables, scalar functions, windows, set operations, and more distinct aggregates are being integrated.</p>
  <Callout type="warn">Active working-tree features are not committed capability claims until their combined parser/operator/fragment suite passes on <code>dev</code>.</Callout>
  <h2>Remaining breadth</h2><p>Scalar and correlated subqueries, comprehensive date/time and decimal behavior, and broad standards conformance remain. Kaveon does not publish an ANSI SQL percentage without a conformance corpus.</p>
  <Pager prev={{ href: "/docs/engine/architecture", title: "Engine Architecture" }} next={{ href: "/docs/engine/distributed", title: "Distributed Runtime" }} />
</div>; }
