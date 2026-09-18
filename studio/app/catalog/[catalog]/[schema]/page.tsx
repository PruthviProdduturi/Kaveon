"use client";

import Link from "next/link";
import { useParams } from "next/navigation";
import { useEffect, useState } from "react";
import { useRole } from "../../../../hooks/useRole";
import { RegisterSheet } from "../../CatalogEditor";
import { tableKey, useCatalogTree } from "../../CatalogShell";
import s from "../../catalog.module.css";
import { enc } from "../../lib";

export default function CatalogSchemaPage() {
  const p = useParams<{ catalog: string; schema: string }>();
  const catalog = decodeURIComponent(p.catalog), schema = decodeURIComponent(p.schema);
  const { catalogs, tables, error, sourceFor, loadTables } = useCatalogTree();
  const { isEditor } = useRole();
  const [adding, setAdding] = useState(false);
  const list = tables[tableKey(catalog, schema)];

  useEffect(() => { loadTables(catalog, schema); }, [catalog, schema, loadTables]);

  const known = catalogs ? !!sourceFor(catalog) : true;

  return (
    <>
      <div className={s.crumbs}><Link href="/catalog">Catalog</Link><i>›</i><b>{catalog}</b><i>›</i><b>{schema}</b></div>
      <header className={s.head}>
        <div>
          <h1 className={s.title}>{schema}</h1>
          <p className={s.subtitle}>{list ? `${list.length} table${list.length === 1 ? "" : "s"} in ${catalog}` : known ? "Reading tables…" : `${catalog} is not a registered catalog.`}</p>
        </div>
        {isEditor && known && (
          <div className={s.actions}>
            <button type="button" className={`${s.ghost} ${s.primary}`} onClick={() => setAdding(true)}><i className="fas fa-plus" aria-hidden="true" /> Add table</button>
          </div>
        )}
      </header>

      {error && <div className={`${s.note} ${s.noteErr}`} role="alert">{error.message}</div>}
      {list && list.length === 0 && (
        <div className={s.empty}>
          <div className={s.emptyTitle}>No tables yet</div>
          <div className={s.emptyBody}>
            {isEditor
              ? "Register a Delta, Iceberg or Parquet table by its location in the catalog's storage. KaveonDB reads it once to verify it before it is added."
              : "This schema has no registered tables. Editors and Administrators can add them."}
          </div>
          {isEditor && <div className={s.emptyActions}><button type="button" className={`${s.ghost} ${s.primary}`} onClick={() => setAdding(true)}><i className="fas fa-plus" aria-hidden="true" /> Add table</button></div>}
        </div>
      )}
      {list && list.length > 0 && (
        <section className={s.panel}>
          <div className={s.grid}>
            <table>
              <thead><tr><th>Table</th><th>Full name</th></tr></thead>
              <tbody>
                {list.map(table => (
                  <tr key={table}>
                    <td><Link href={`/catalog/${enc(catalog)}/${enc(schema)}/${enc(table)}`} style={{ color: "var(--accent)", textDecoration: "none" }}><i className="fas fa-table" aria-hidden="true" style={{ color: "var(--text-faint)", marginRight: 8 }} />{table}</Link></td>
                    <td className={s.type}>{catalog}.{schema}.{table}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>
      )}

      {adding && <RegisterSheet kind="table" catalog={catalog} schema={schema} onClose={() => setAdding(false)} />}
    </>
  );
}
