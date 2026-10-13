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
 * into inside a list whose header offers registration. It is reached by its
 * Engine identifier, which is the one name the page that opens it resolves.
 *
 * What counts as a catalog here is the registry, not the Engine.
 *
 * A catalog source record is what an administrator registers, what carries
 * the draft → active lifecycle, and what every other surface resolves a
 * catalog through: the rail beside this page, the table page, and SQL Lab all
 * read `fetchSources`. The Engine's catalog definitions are the layer
 * underneath, and they hold whatever has been written straight into the
 * Engine as well — a benchmark campaign registers a scratch catalog per run.
 * Listing definitions therefore labelled that scratch "Registered catalog"
 * and put a count on this page that no other surface agreed with.
 *
 * So the list is the registry, and the Engine's definitions are joined to it
 * for the one thing only they carry: the catalog id a schema read needs. A
 * catalog has to be in both to be listed, because one without the other is a
 * disagreement rather than a catalog: a definition nobody registered cannot
 * be queried, and a registration the Engine does not define cannot be opened.
 * Neither is hidden — both are reported by name to the administrator who can
 * act on them, since quietly dropping them would make this page misleading in
 * the other direction.
 */

import Link from "next/link";
import { useEffect, useMemo, useState } from "react";
import { useRole } from "../../hooks/useRole";
import { RegisterSheet } from "./CatalogEditor";
import { useCatalogTree } from "./CatalogShell";
import {
  CatalogDefinition, EngineSource, SchemaDefinition, enc, fetchDefinitions, fetchSchemaDefinitions,
  isSystemCatalogName,
} from "./lib";
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

/** One listed catalog: the registry row, and the Engine definition behind it. */
interface Registered { source: EngineSource; definition: CatalogDefinition }

function CatalogIndex({ onAdd }: { onAdd?: () => void }) {
  const { isAdmin } = useRole();
  // The same read the rail made, through the shell, so the two cannot differ
  // and the page costs no second request for it.
  const { catalogs: registry, error: registryError } = useCatalogTree();
  const [definitions, setDefinitions] = useState<CatalogDefinition[] | null>(null);
  const [schemas, setSchemas] = useState<Record<string, SchemaDefinition[]>>({});
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    void fetchDefinitions()
      .then(list => { if (alive) { setDefinitions(list); setError(null); } })
      .catch(() => { if (alive) setError("Catalogs could not be read."); });
    return () => { alive = false; };
  }, []);

  /** The registry joined to the Engine, by the catalog name both sides hold. */
  const joined = useMemo(() => {
    if (!registry || !definitions) return null;
    const byName = new Map(definitions.map(def => [def.name.toLowerCase(), def]));
    return registry
      .filter(source => !isSystemCatalogName(source.catalog))
      .map(source => ({ source, definition: byName.get(source.catalog.toLowerCase()) ?? null }));
  }, [registry, definitions]);

  /** Registered, and the Engine defines it: a catalog a reader can open and query. */
  const listed = useMemo<Registered[] | null>(() => joined === null ? null
    : joined.flatMap(entry => entry.definition ? [{ source: entry.source, definition: entry.definition }] : []),
    [joined]);

  /** Registered, but the Engine holds no definition for it. */
  const undefinedSources = useMemo(() => (joined ?? []).filter(entry => !entry.definition).map(entry => entry.source), [joined]);

  /** In the Engine, with nothing in the registry: scratch, or a catalog never registered. */
  const unregistered = useMemo(() => {
    if (!registry || !definitions) return [];
    const registeredNames = new Set(registry.map(source => source.catalog.toLowerCase()));
    return definitions.filter(def =>
      !isSystemCatalogName(def.name) && !registeredNames.has(def.name.toLowerCase()));
  }, [registry, definitions]);

  // Schemas are read per listed catalog, and only for the listed ones. The
  // ids are joined into a string so the effect depends on which catalogs are
  // listed rather than on a fresh array every render.
  const idKey = useMemo(() => (listed ?? []).map(entry => entry.definition.id).join(" "), [listed]);
  useEffect(() => {
    const ids = idKey ? idKey.split(" ") : [];
    if (!ids.length) return;
    let alive = true;
    void Promise.all(ids.map(async id =>
      [id, await fetchSchemaDefinitions(id).catch(() => [] as SchemaDefinition[])] as const))
      .then(pairs => { if (alive) setSchemas(Object.fromEntries(pairs)); });
    return () => { alive = false; };
  }, [idKey]);

  const totalSchemas = useMemo(
    () => (listed ?? []).reduce((sum, entry) => sum + (schemas[entry.definition.id]?.length ?? 0), 0),
    [listed, schemas]);
  // The platform's own catalog is shown because the Engine defines it, never
  // on the page's word: an installation without it must not be told it has one.
  const system = definitions?.find(def => isSystemCatalog(def.name)) ?? null;
  const failure = error ?? registryError?.message ?? null;

  return (
    <main className={s.index}>
      <header className={s.indexHeader}>
        <div>
          <div className={s.eyebrow}><i className="fas fa-layer-group" aria-hidden="true" /> Data catalog</div>
          <h1 className={s.indexTitle}>Your data, by catalog</h1>
          <p className={s.indexLead}>A catalog is listed here once it is registered as a source, which is also what makes it queryable in SQL Lab. Open one to inspect its schemas, tables, locations, and measurements.</p>
        </div>
        <div className={s.indexActions}>
          <span className={s.indexStat}><b>{listed?.length ?? "—"}</b><small>catalogs</small></span>
          <span className={s.indexStat}><b>{listed ? totalSchemas : "—"}</b><small>schemas</small></span>
          {onAdd && <button type="button" className={`${s.ghost} ${s.primary}`} onClick={onAdd}><i className="fas fa-plus" aria-hidden="true" /> Add schema</button>}
        </div>
      </header>
      {failure && <p className={s.indexError}>{failure}</p>}
      <section className={s.catalogGrid} aria-label="Registered catalogs">
        {listed === null && !failure && [0, 1, 2].map(i => <div className={s.catalogCardSkeleton} key={i} />)}
        {system && <Link className={`${s.catalogCard} ${s.catalogCardSystem}`} href={`/catalog/${enc(system.name)}`}>
          <span className={s.catalogIcon}><i className="fas fa-database" aria-hidden="true" /></span>
          <span className={s.catalogCardBody}><span className={s.catalogCardKicker}>System catalog</span><strong>{SYSTEM_CATALOG_LABEL}</strong><span>Transactional product records, audit history, and platform metadata.</span></span>
          <i className={`fas fa-arrow-up-right-from-square ${s.catalogArrow}`} aria-hidden="true" />
        </Link>}
        {listed?.map(({ source, definition }) => {
          const list = schemas[definition.id];
          return <Link className={s.catalogCard} href={`/catalog/${enc(definition.name)}`} key={source.id}>
            <span className={s.catalogIcon}><i className="fas fa-database" aria-hidden="true" /></span>
            <span className={s.catalogCardBody}><span className={s.catalogCardKicker}>Registered catalog</span><strong>{definition.name}</strong><span>{list ? `${list.length} ${list.length === 1 ? "schema" : "schemas"}` : "Loading schemas…"} · Open to inspect tables</span></span>
            <i className={`fas fa-arrow-up-right-from-square ${s.catalogArrow}`} aria-hidden="true" />
          </Link>;
        })}
      </section>
      {listed?.length === 0 && !failure && <div className={s.indexEmpty}>No registered catalogs yet.</div>}
      <Reconcile isAdmin={isAdmin} unregistered={unregistered} undefinedSources={undefinedSources} />
    </main>
  );
}

/**
 * Where the registry and the Engine disagree.
 *
 * Not a catalog list — a reconciliation, for the one role that can settle it.
 * It is collapsed and it is last, because neither kind of entry is something
 * a reader can open: the point is that an administrator can see what is in
 * the Engine without it being presented to everybody as part of the product's
 * data. A reader without the Admin role sees nothing here, since registering
 * and synchronizing catalogs is an Admin action and the names are of no use
 * to anybody who cannot take it.
 */
function Reconcile({ isAdmin, unregistered, undefinedSources }: {
  isAdmin: boolean; unregistered: CatalogDefinition[]; undefinedSources: EngineSource[];
}) {
  if (!isAdmin || (!unregistered.length && !undefinedSources.length)) return null;
  const parts = [
    unregistered.length
      ? `${unregistered.length} Engine ${unregistered.length === 1 ? "definition" : "definitions"} not registered`
      : null,
    undefinedSources.length
      ? `${undefinedSources.length} ${undefinedSources.length === 1 ? "registration" : "registrations"} the Engine does not define`
      : null,
  ].filter(Boolean);

  return (
    <details className={s.indexReconcile}>
      <summary><i className="fas fa-circle-info" aria-hidden="true" />{parts.join(" · ")}</summary>
      {unregistered.length > 0 && (
        <div className={s.reconcileBlock}>
          <p>
            These catalogs exist in the Engine but have no registered source, so nothing in
            Kaveon can query them and they are not listed above. Definitions written straight
            into the Engine — a benchmark campaign registers one per run — are left behind this
            way. Register one to make it queryable, or remove it from the Engine.
          </p>
          <ul className={s.reconcileNames}>
            {unregistered.map(def => <li key={def.id}><code>{def.name}</code></li>)}
          </ul>
        </div>
      )}
      {undefinedSources.length > 0 && (
        <div className={s.reconcileBlock}>
          <p>
            These are registered, but the Engine holds no catalog definition for them, so there
            is nothing to open or query yet. Synchronize the source to the Engine from the
            Catalog sources screen.
          </p>
          <ul className={s.reconcileNames}>
            {undefinedSources.map(source => <li key={source.id}><code>{source.catalog}</code></li>)}
          </ul>
        </div>
      )}
      <Link href="/settings/storage" className={s.reconcileLink}>
        <i className="fas fa-sliders" aria-hidden="true" /> Catalog sources
      </Link>
    </details>
  );
}
