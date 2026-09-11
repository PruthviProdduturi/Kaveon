"use client";

import Link from "next/link";
import { useRole } from "../../hooks/useRole";
import { useCatalogTree } from "./CatalogShell";
import s from "./catalog.module.css";
import { enc } from "./lib";

export default function CatalogOverviewPage() {
  const { catalogs, schemas, error } = useCatalogTree();
  const { isAdmin } = useRole();

  if (error) {
    return <div className={`${s.note} ${s.noteErr}`} role="alert"><b>The catalog is unavailable.</b> {error.message}</div>;
  }

  if (catalogs && catalogs.length === 0) {
    return (
      <div className={s.empty}>
        <div className={s.emptyTitle}>No catalogs yet</div>
        <div className={s.emptyBody}>
          Register a storage location under Settings → Storage and synchronize it with KaveonDB. Its schemas and tables appear here and in SQL Lab.
        </div>
        {isAdmin && <div className={s.emptyActions}><Link href="/settings/storage" className={`${s.ghost} ${s.primary}`}>Register a catalog source</Link></div>}
      </div>
    );
  }

  return (
    <>
      <header className={s.head}>
        <div>
          <h1 className={s.title} style={{ fontFamily: "inherit" }}>Catalog</h1>
          <p className={s.subtitle}>Every table here is read where it lives in your lake. Open one for its columns, location, format and a sample of rows — then query it in SQL Lab.</p>
        </div>
        {isAdmin && <div className={s.actions}><Link href="/settings/storage" className={s.ghost}><i className="fas fa-sliders" aria-hidden="true" /> Catalog sources</Link></div>}
      </header>

      {(catalogs ?? []).map(cat => {
        const list = schemas[cat.catalog];
        return (
          <section key={cat.catalog} className={s.panel}>
            <div className={s.panelHead}>
              <h2 className={s.panelTitle}><i className="fas fa-database" aria-hidden="true" style={{ color: "var(--text-faint)", marginRight: 8 }} />{cat.catalog}</h2>
              <span className={s.panelMeta}>{list ? `${list.length} schema${list.length === 1 ? "" : "s"}` : "loading…"}</span>
            </div>
            {list && list.length === 0 && <div className={s.note}>No schemas are registered in this catalog.</div>}
            {list && list.length > 0 && (
              <div className={s.chips}>
                {list.map(schema => (
                  <Link key={schema} href={`/catalog/${enc(cat.catalog)}/${enc(schema)}`} className={s.chip} style={{ textDecoration: "none" }}>
                    <i className="fas fa-folder" aria-hidden="true" style={{ color: "var(--text-faint)" }} />{schema}
                  </Link>
                ))}
              </div>
            )}
          </section>
        );
      })}
      {!catalogs && <div className={s.note}>Loading catalogs…</div>}
    </>
  );
}
