"use client";

import Link from "next/link";
import { useSearchParams } from "next/navigation";
import { LabWorkbench } from "../../lab/LabWorkbench";
import { useCatalogTree } from "../CatalogShell";
import s from "../catalog.module.css";

// SQL Lab as the query mode of the Catalog: the shell's tree stays on the
// left; the workbench takes the pane. Catalog and schema come from the URL so
// a table page can hand off exactly the context it was showing.
export default function CatalogQueryPage() {
  const search = useSearchParams();
  const { catalogs, sourceFor } = useCatalogTree();
  const catalog = search.get("catalog") ?? catalogs?.[0]?.catalog ?? null;
  const schema = search.get("schema");
  const source = catalog ? sourceFor(catalog) : null;

  return (
    <div className={s.query}>
      <div className={s.crumbs}>
        <Link href="/catalog">Catalog</Link><i>›</i><b>SQL Lab</b>
        {catalog && <><i>·</i><span>{catalog}{schema ? `.${schema}` : ""}</span></>}
      </div>
      {catalogs && catalogs.length === 0 ? (
        <div className={s.note}>No KaveonDB catalogs are registered. Register one under Settings → Storage to query it here.</div>
      ) : (
        <div className={s.workbench}>
          <LabWorkbench embedded engineSourceId={source?.id ?? null} schema={schema} />
        </div>
      )}
    </div>
  );
}
