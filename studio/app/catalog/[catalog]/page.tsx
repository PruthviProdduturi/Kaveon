"use client";

import Link from "next/link";
import { useParams } from "next/navigation";
import { useState } from "react";
import { useRole } from "../../../hooks/useRole";
import { RegisterSheet } from "../CatalogEditor";
import { Inventory } from "../Inventory";
import { SystemCatalog } from "../SystemCatalog";
import { useCatalogTree } from "../CatalogShell";
import { isSystemCatalogName } from "../lib";
import s from "../catalog.module.css";

export default function CatalogPage() {
  const params = useParams<{ catalog: string }>();
  const catalog = params.catalog ? decodeURIComponent(params.catalog) : "";
  const { catalogs: registry } = useCatalogTree();
  const { isEditor } = useRole();
  const [adding, setAdding] = useState(false);
  const [reload, setReload] = useState(0);

  // Either of the system catalog's two names opens its own reading, so a link
  // written to the product label resolves as well as one to the identifier.
  if (isSystemCatalogName(catalog)) return <SystemCatalog />;

  // The same rule the table page already states, applied where a reader can
  // arrive at a catalog by typing or following an old link. A name the Engine
  // defines but the registry does not hold is not a catalog this product has:
  // it cannot be queried, so it is told so plainly rather than opening an
  // inventory whose "Query in SQL Lab" links the Lab would refuse.
  const unregistered = registry !== null
    && !registry.some(source => source.catalog.toLowerCase() === catalog.toLowerCase());

  if (unregistered) {
    return (
      <div className={s.empty}>
        <h1 className={s.emptyTitle}>{catalog} is not a registered catalog</h1>
        <p className={s.emptyBody}>
          A catalog becomes part of Kaveon when it is registered as a catalog source, which is
          what makes it readable here and queryable in SQL Lab. The Engine may hold a definition
          by this name — one written directly into it, for instance by a benchmark run — but
          nothing registered answers to it.
        </p>
        <div className={s.emptyActions}>
          <Link href="/catalog" className={`${s.ghost} ${s.primary}`}>Registered catalogs</Link>
        </div>
      </div>
    );
  }

  // A schema is the one thing a catalog's own page can be missing, and the
  // overview above and the schema page below both offer to add one. This level
  // did not, so an editor who had navigated into a catalog had to go back out
  // to register into it. The sheet opens scoped to this catalog.
  return (
    <>
      <Inventory
        catalogName={catalog} reloadToken={reload}
        action={isEditor && (
          <button type="button" className={`${s.ghost} ${s.primary}`} onClick={() => setAdding(true)}>
            <i className="fas fa-plus" aria-hidden="true" /> Add schema
          </button>
        )}
      />
      {adding && (
        <RegisterSheet
          kind="schema" catalog={catalog}
          onClose={() => { setAdding(false); setReload(count => count + 1); }}
        />
      )}
    </>
  );
}
