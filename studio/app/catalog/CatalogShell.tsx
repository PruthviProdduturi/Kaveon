"use client";

import Link from "next/link";
import { useParams, usePathname } from "next/navigation";
import { createContext, useCallback, useContext, useEffect, useMemo, useState } from "react";
import s from "./catalog.module.css";
import { CatalogError, EngineSource, enc, fetchSchemas, fetchSources, fetchTables } from "./lib";

// The product speaks in catalogs. A "source" is the registry row behind a
// catalog and never appears in a URL or a label; the shell resolves it.
interface TreeState {
  catalogs: EngineSource[] | null;
  schemas: Record<string, string[] | undefined>;   // by catalog name
  tables: Record<string, string[] | undefined>;    // by "catalog schema"
  error: CatalogError | null;
  sourceFor: (catalog: string) => EngineSource | null;
  loadSchemas: (catalog: string) => Promise<void>;
  loadTables: (catalog: string, schema: string) => Promise<void>;
}

const TreeContext = createContext<TreeState | null>(null);
export const useCatalogTree = () => {
  const ctx = useContext(TreeContext);
  if (!ctx) throw new Error("useCatalogTree must be used inside CatalogShell");
  return ctx;
};

export const tableKey = (catalog: string, schema: string) => `${catalog} ${schema}`;

export function CatalogShell({ children }: { children: React.ReactNode }) {
  const pathname = usePathname();
  const params = useParams<{ catalog?: string; schema?: string; table?: string }>();
  const current = {
    catalog: params.catalog ? decodeURIComponent(params.catalog) : null,
    schema: params.schema ? decodeURIComponent(params.schema) : null,
    table: params.table ? decodeURIComponent(params.table) : null,
  };

  const [catalogs, setCatalogs] = useState<EngineSource[] | null>(null);
  const [schemas, setSchemas] = useState<Record<string, string[] | undefined>>({});
  const [tables, setTables] = useState<Record<string, string[] | undefined>>({});
  const [error, setError] = useState<CatalogError | null>(null);
  const [open, setOpen] = useState<Set<string>>(new Set());
  const [collapsed, setCollapsed] = useState(false);

  const sourceFor = useCallback((catalog: string) => catalogs?.find(c => c.catalog === catalog) ?? null, [catalogs]);
  const fail = (e: unknown) => setError(e instanceof CatalogError ? e : new CatalogError(0, "The catalog could not be read."));

  const loadSchemas = useCallback(async (catalog: string) => {
    const src = sourceFor(catalog);
    if (!src || schemas[catalog]) return;
    try { const list = await fetchSchemas(src.id); setSchemas(prev => ({ ...prev, [catalog]: list })); } catch (e) { fail(e); }
  }, [sourceFor, schemas]);

  const loadTables = useCallback(async (catalog: string, schema: string) => {
    const src = sourceFor(catalog), k = tableKey(catalog, schema);
    if (!src || tables[k]) return;
    try { const list = await fetchTables(src.id, schema); setTables(prev => ({ ...prev, [k]: list })); } catch (e) { fail(e); }
  }, [sourceFor, tables]);

  useEffect(() => {
    let cancelled = false;
    fetchSources()
      .then(list => { if (!cancelled) { setCatalogs(list); setError(null); setOpen(new Set(list.map(c => c.catalog))); } })
      .catch(e => { if (!cancelled) fail(e); });
    return () => { cancelled = true; };
  }, []);

  // Catalogs are few and always worth seeing open; load their schema lists once known.
  useEffect(() => { catalogs?.forEach(c => { loadSchemas(c.catalog); }); }, [catalogs, loadSchemas]);

  // A deep link lands expanded along its own path.
  useEffect(() => {
    if (!current.catalog) return;
    setOpen(prev => {
      const next = new Set(prev); next.add(current.catalog!);
      if (current.schema) next.add(tableKey(current.catalog!, current.schema));
      return next;
    });
    if (current.schema) loadTables(current.catalog, current.schema);
  }, [current.catalog, current.schema, loadTables]);

  const toggle = (id: string, load?: () => void) => {
    setOpen(prev => { const next = new Set(prev); if (next.has(id)) next.delete(id); else next.add(id); return next; });
    load?.();
  };

  const value = useMemo<TreeState>(() => ({ catalogs, schemas, tables, error, sourceFor, loadSchemas, loadTables }),
    [catalogs, schemas, tables, error, sourceFor, loadSchemas, loadTables]);

  return (
    <TreeContext.Provider value={value}>
      <div className={`page-shell ${s.root} ${collapsed ? s.rootCollapsed : ""}`}>
        <aside className={s.tree} aria-label="Catalogs">
          {collapsed ? (
            <div className={s.rail}>
              <button type="button" className={s.collapse} onClick={() => setCollapsed(false)} aria-label="Expand catalog tree"><i className="fas fa-angles-right" /></button>
            </div>
          ) : (
            <>
              <div className={s.treeHead}>
                <h2 className={s.treeTitle}>Catalogs{catalogs && <span>{catalogs.length}</span>}</h2>
                <button type="button" className={s.collapse} onClick={() => setCollapsed(true)} aria-label="Collapse catalog tree"><i className="fas fa-angles-left" /></button>
              </div>
              <div className={s.treeBody}>
                {!catalogs && !error && <div className={s.treeNote}>Loading catalogs…</div>}
                {error && <div className={`${s.treeNote} ${s.treeErr}`}>{error.message}</div>}
                {catalogs && catalogs.length === 0 && <div className={s.treeNote}>No KaveonDB catalogs are registered.</div>}
                {catalogs?.map(cat => {
                  const name = cat.catalog, isOpen = open.has(name), list = schemas[name];
                  return (
                    <div key={name}>
                      <button type="button" className={s.node} aria-expanded={isOpen} onClick={() => toggle(name, () => loadSchemas(name))}>
                        <span className={`${s.chev} ${isOpen ? s.chevOpen : ""}`}>▶</span>
                        <span className={s.kind}><i className="fas fa-database" /></span>
                        <span className={s.nodeLabel}>{name}</span>
                        {list && <span className={s.nodeCount}>{list.length}</span>}
                      </button>
                      {isOpen && (list ?? []).map(schema => {
                        const k = tableKey(name, schema), sOpen = open.has(k), tlist = tables[k];
                        const schemaActive = current.catalog === name && current.schema === schema && !current.table;
                        return (
                          <div key={k}>
                            <Link href={`/catalog/${enc(name)}/${enc(schema)}`} className={`${s.node} ${s.level1}`} aria-current={schemaActive ? "page" : undefined}
                              onClick={() => { setOpen(prev => new Set(prev).add(k)); loadTables(name, schema); }}>
                              <span className={`${s.chev} ${sOpen ? s.chevOpen : ""}`}>▶</span>
                              <span className={s.kind}><i className="fas fa-folder" /></span>
                              <span className={s.nodeLabel}>{schema}</span>
                              {tlist && <span className={s.nodeCount}>{tlist.length}</span>}
                            </Link>
                            {sOpen && (tlist ?? []).map(table => {
                              const active = current.catalog === name && current.schema === schema && current.table === table;
                              return (
                                <Link key={table} href={`/catalog/${enc(name)}/${enc(schema)}/${enc(table)}`} className={`${s.node} ${s.level2}`} aria-current={active ? "page" : undefined}>
                                  <span className={s.chev} />
                                  <span className={s.kind}><i className="fas fa-table" /></span>
                                  <span className={s.nodeLabel}>{table}</span>
                                </Link>
                              );
                            })}
                            {sOpen && tlist && tlist.length === 0 && <div className={`${s.treeNote} ${s.level2}`}>No tables</div>}
                          </div>
                        );
                      })}
                      {isOpen && list && list.length === 0 && <div className={`${s.treeNote} ${s.level1}`}>No schemas</div>}
                    </div>
                  );
                })}
              </div>
              <div className={s.treeFoot}>
                <Link href={current.catalog ? `/catalog/query?catalog=${enc(current.catalog)}${current.schema ? `&schema=${enc(current.schema)}` : ""}` : "/catalog/query"} className={s.ghost} aria-current={pathname.startsWith("/catalog/query") ? "page" : undefined}><i className="fas fa-code" aria-hidden="true" /> SQL Lab</Link>
              </div>
            </>
          )}
        </aside>
        <div className={s.pane} key={pathname}>{children}</div>
      </div>
    </TreeContext.Provider>
  );
}
