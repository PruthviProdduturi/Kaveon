"use client";

/**
 * One schema — the same inventory, narrowed to the tables inside it, so the
 * list a reader learned on the Catalog page is the list they land on here.
 */

import Link from "next/link";
import { useParams } from "next/navigation";
import { useState } from "react";
import { useRole } from "../../../../hooks/useRole";
import { RegisterSheet } from "../../CatalogEditor";
import { Inventory } from "../../Inventory";
import s from "../../catalog.module.css";

export default function CatalogSchemaPage() {
  const p = useParams<{ catalog: string; schema: string }>();
  const catalog = decodeURIComponent(p.catalog), schema = decodeURIComponent(p.schema);
  const { isEditor } = useRole();
  const [adding, setAdding] = useState(false);
  // The sheet registers the table on the Engine; this list reads the Engine's
  // definitions. Closing it re-reads them, so the table that was just added
  // is in the list the reader is returned to.
  const [reload, setReload] = useState(0);

  return (
    <>
      <div className={s.crumbs}>
        <Link href="/catalog">Catalog</Link><i>›</i><b>{catalog}</b><i>›</i><b>{schema}</b>
      </div>
      <Inventory
        catalogName={catalog} schemaName={schema} reloadToken={reload}
        action={isEditor && (
          <button type="button" className={`${s.ghost} ${s.primary}`} onClick={() => setAdding(true)}>
            <i className="fas fa-plus" aria-hidden="true" /> Add table
          </button>
        )}
      />
      {adding && (
        <RegisterSheet
          kind="table" catalog={catalog} schema={schema}
          onClose={() => { setAdding(false); setReload(count => count + 1); }}
        />
      )}
    </>
  );
}
