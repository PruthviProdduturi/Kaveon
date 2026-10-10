"use client";

/**
 * Catalog — the inventory of everything Kaveon can answer over.
 *
 * The page is the table list. Registration lives beside it, because a schema
 * is the only thing this list can be missing: the action appears once, in the
 * header of the list it adds to.
 *
 * KaveonDB follows the list rather than joining it. It is a catalog, but not
 * one anybody puts a table in: it holds the platform's own records, it is
 * read-only from here, and its figures have nothing to do with the lake the
 * list above measures. Mixing it in would mean a schema nobody may register
 * into inside a list whose header offers registration.
 */

import Link from "next/link";
import { useEffect, useMemo, useState } from "react";
import { useRole } from "../../hooks/useRole";
import { RegisterSheet } from "./CatalogEditor";
import { fetchDefinitions, fetchSchemaDefinitions, CatalogDefinition, SchemaDefinition, enc } from "./lib";
import { isSystemCatalog, SYSTEM_CATALOG_LABEL } from "../../utils/systemCatalog";
import s from "./catalog.module.css";

export default function CatalogOverviewPage() {
  const { isEditor } = useRole();
  const [adding, setAdding] = useState(false);

  return (
    <>
      <CatalogIndex onAdd={isEditor ? () => setAdding(true) : undefined} />
      {adding && <RegisterSheet kind="schema" onClose={() => setAdding(false)} />}
    </>
  );
}

function CatalogIndex({ onAdd }: { onAdd?: () => void }) {
  const [definitions, setDefinitions] = useState<CatalogDefinition[] | null>(null);
  const [schemas, setSchemas] = useState<Record<string, SchemaDefinition[]>>({});
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    void fetchDefinitions().then(async list => {
      if (!alive) return;
      const visible = list.filter(def => !isSystemCatalog(def.name));
      setDefinitions(visible);
      const pairs = await Promise.all(visible.map(async def => [def.id, await fetchSchemaDefinitions(def.id).catch(() => [])] as const));
      if (alive) setSchemas(Object.fromEntries(pairs));
    }).catch(() => { if (alive) setError("Catalogs could not be read."); });
    return () => { alive = false; };
  }, []);

  const totalSchemas = useMemo(() => Object.values(schemas).reduce((sum, list) => sum + list.length, 0), [schemas]);
  const system = definitions === null ? null : "KaveonDB";

  return (
    <main className={s.index}>
      <header className={s.indexHeader}>
        <div>
          <div className={s.eyebrow}><i className="fas fa-layer-group" aria-hidden="true" /> Data catalog</div>
          <h1 className={s.indexTitle}>Your data, by catalog</h1>
          <p className={s.indexLead}>A single overview of every registered source. Open a catalog to inspect its schemas, tables, locations, and measurements.</p>
        </div>
        <div className={s.indexActions}>
          <span className={s.indexStat}><b>{definitions?.length ?? "—"}</b><small>catalogs</small></span>
          <span className={s.indexStat}><b>{definitions ? totalSchemas : "—"}</b><small>schemas</small></span>
          {onAdd && <button type="button" className={`${s.ghost} ${s.primary}`} onClick={onAdd}><i className="fas fa-plus" aria-hidden="true" /> Add schema</button>}
        </div>
      </header>
      {error && <p className={s.indexError}>{error}</p>}
      <section className={s.catalogGrid} aria-label="Registered catalogs">
        {definitions === null && [0, 1, 2].map(i => <div className={s.catalogCardSkeleton} key={i} />)}
        {system && <Link className={`${s.catalogCard} ${s.catalogCardSystem}`} href={`/catalog/${enc(system)}`}>
          <span className={s.catalogIcon}><i className="fas fa-database" aria-hidden="true" /></span>
          <span className={s.catalogCardBody}><span className={s.catalogCardKicker}>System catalog</span><strong>{SYSTEM_CATALOG_LABEL}</strong><span>Transactional product records, audit history, and platform metadata.</span></span>
          <i className={`fas fa-arrow-up-right-from-square ${s.catalogArrow}`} aria-hidden="true" />
        </Link>}
        {definitions?.map(def => {
          const list = schemas[def.id];
          return <Link className={s.catalogCard} href={`/catalog/${enc(def.name)}`} key={def.id}>
            <span className={s.catalogIcon}><i className="fas fa-database" aria-hidden="true" /></span>
            <span className={s.catalogCardBody}><span className={s.catalogCardKicker}>Registered catalog</span><strong>{def.name}</strong><span>{list ? `${list.length} ${list.length === 1 ? "schema" : "schemas"}` : "Loading schemas…"} · Open to inspect tables</span></span>
            <i className={`fas fa-arrow-up-right-from-square ${s.catalogArrow}`} aria-hidden="true" />
          </Link>;
        })}
      </section>
      {definitions?.length === 0 && !error && <div className={s.indexEmpty}>No registered catalogs yet.</div>}
    </main>
  );
}
