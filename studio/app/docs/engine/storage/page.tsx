import { Callout, PageHeader, Pager } from "../../../../components/docs/prose";

export const metadata = { title: "Storage and Catalogs" };
export default function StorageCatalogDocs() { return <div className="docs-prose">
  <PageHeader eyebrow="Engine" title="Storage and catalogs" lead="Engine streams Arrow batches directly from registered lake data and resolves immutable worker inputs through its native catalog." />
  <h2>Reads</h2><p>Local, ADLS Gen2, and S3 Parquet readers preserve requested column order and conservatively prune row groups. Delta supports contiguous JSON history and complete classic or multipart checkpoints, with coordinator-pinned versions. Empty Delta tables retain their declared schema. Iceberg v1/v2 readers accept a committed metadata JSON pointer and read supported Parquet snapshots by field ID. Deterministic splits feed distributed scans; row-level filters preserve correctness.</p>
  <h2>Catalog</h2><p>SQLite/WAL metadata provides transactions, migrations, stable IDs, optimistic revisions, structured Arrow schemas, lifecycle enforcement, credential references, and audit history. Workers use coordinator-resolved fragment locations.</p>
  <Callout type="warn">Cloud readers have local object-store tests; live cloud deployment qualification remains pending. Delta partition reconstruction, column mapping, deletion vectors, and unsupported schema evolution fail explicitly. Iceberg active deletes and nested/default fields are unsupported. Platform and Engine catalogs remain separate stores connected through explicit revision-checked synchronization; external catalog adapters remain targets.</Callout>
  <Pager prev={{ href: "/docs/engine/distributed", title: "Distributed Runtime" }} next={{ href: "/docs/memory", title: "Engine Memory" }} />
</div>; }
