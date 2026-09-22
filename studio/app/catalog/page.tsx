"use client";

import Link from "next/link";
import { useState } from "react";
import { useRole } from "../../hooks/useRole";
import { RegisterSheet } from "./CatalogEditor";
import { catalogLabel, useCatalogTree } from "./CatalogShell";
import s from "./catalog.module.css";
import { enc } from "./lib";

export default function CatalogOverviewPage() {
  const { catalogs, schemas, error } = useCatalogTree();
  const { isAdmin, isEditor } = useRole();
  const [adding, setAdding] = useState<{ catalog?: string } | null>(null);

  if (error) {
    return <div className={`${s.note} ${s.noteErr}`} role="alert"><b>The catalog is unavailable.</b> {error.message}</div>;
  }

  const noSchemas = !!catalogs && catalogs.length > 0 && catalogs.every(c => schemas[c.catalog]?.length === 0);

  if (catalogs && catalogs.length === 0) {
    return (
      <div className={s.empty}>
        <div className={s.emptyTitle}>No catalogs yet</div>
        <div className={s.emptyBody}>
          A catalog is a storage location KaveonDB reads in place — an ADLS Gen2 container, a local directory or an S3 bucket. Register one under Settings → Storage and synchronize it; then add schemas and tables here, and they appear in SQL Lab.
        </div>
        {isAdmin && <div className={s.emptyActions}><Link href="/settings/storage" className={`${s.ghost} ${s.primary}`}>Register a catalog</Link></div>}
      </div>
    );
  }

  return (
    <>
      <header className={s.head}>
        <div>
            <h1 className={s.title} style={{ fontFamily: "inherit" }}>KaveonDB</h1>
            <p className={s.subtitle}>Your governed data surface. Open a catalog to inspect its schemas and tables, then query the same definitions in SQL Lab.</p>
        </div>
        <div className={s.actions}>
          {isAdmin && <Link href="/settings/storage" className={s.ghost}><i className="fas fa-sliders" aria-hidden="true" /> Catalog sources</Link>}
          {isEditor && !noSchemas && <button type="button" className={`${s.ghost} ${s.primary}`} onClick={() => setAdding({})}><i className="fas fa-plus" aria-hidden="true" /> Add schema</button>}
        </div>
      </header>

      {noSchemas && (
        <div className={s.empty} style={{ marginBottom: 14 }}>
          <div className={s.emptyTitle}>No schemas yet</div>
          <div className={s.emptyBody}>
            {isEditor
              ? "Add a schema to a catalog, then register the Delta, Iceberg or Parquet tables under it. Each table is verified on KaveonDB before it is added."
              : "Schemas and tables are added by Editors and Administrators. Once registered, they appear here and in SQL Lab."}
          </div>
          {isEditor && <div className={s.emptyActions}><button type="button" className={`${s.ghost} ${s.primary}`} onClick={() => setAdding({})}><i className="fas fa-plus" aria-hidden="true" /> Add schema</button></div>}
        </div>
      )}

      {(catalogs ?? []).map(cat => {
        const list = schemas[cat.catalog];
        return (
          <section key={cat.catalog} className={s.panel}>
            <div className={s.panelHead}>
              <h2 className={s.panelTitle}><i className="fas fa-database" aria-hidden="true" style={{ color: "var(--text-faint)", marginRight: 8 }} />{catalogLabel(cat.catalog)}</h2>
              <span className={s.panelMeta}>
                {list ? `${list.length} schema${list.length === 1 ? "" : "s"}` : "loading…"}
                {isEditor && list && <button type="button" className={s.ghost} style={{ height: 24, padding: "0 8px", marginLeft: 12, fontSize: 11.5 }} onClick={() => setAdding({ catalog: cat.catalog })}><i className="fas fa-plus" aria-hidden="true" /> Add schema</button>}
              </span>
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

      {adding && <RegisterSheet kind="schema" catalog={adding.catalog} onClose={() => setAdding(null)} />}
    </>
  );
}
