import { Callout, PageHeader, Pager } from "../../../../components/docs/prose";

export const metadata = { title: "Storage and Catalogs" };
export default function StorageCatalogDocs() { return <div className="docs-prose">
  <PageHeader eyebrow="Engine Manual" title="Storage and catalogs" lead="Engine streams Arrow batches directly from registered lake data and resolves immutable worker inputs through its native catalog." />
  <h2>Reads</h2><p>Parquet projection is strict and row-group pruning is conservative. Local Delta replays contiguous JSON history from version zero and reads active files. Deterministic row-group/file splits feed distributed scans, while row-level filters preserve correctness.</p>
  <h2>Catalog</h2><p>SQLite/WAL metadata provides transactions, migrations, stable IDs, optimistic revisions, structured Arrow schemas, lifecycle enforcement, credential references, and audit history. Workers use coordinator-resolved fragment locations.</p>
  <Callout type="warn">The platform PostgreSQL source registry and Engine catalog remain separate authorities. ADLS Gen2, S3, Delta checkpoints/deletion vectors, Iceberg, and external catalog adapters remain pending.</Callout>
  <Pager prev={{ href: "/docs/engine/distributed", title: "Distributed Runtime" }} next={{ href: "/docs/memory", title: "Engine Memory" }} />
</div>; }
