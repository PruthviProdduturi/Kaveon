"use client";

import Link from "next/link";
import { useParams, usePathname } from "next/navigation";
import { createContext, useCallback, useContext, useEffect, useMemo, useState } from "react";
import { useRole } from "../../hooks/useRole";
import {
  SYSTEM_CATALOG_LABEL, SYSTEM_CATALOG_SCHEMA, SystemTable, catalogLabel, fetchSystemCatalog,
  isSystemCatalog,
} from "../../utils/systemCatalog";
import s from "./catalog.module.css";
import {
  CatalogError, EngineSource, enc, fetchDefinitions, fetchSchemaDefinitions,
  fetchSchemas, fetchSources, fetchTables,
} from "./lib";

export { catalogLabel };

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
  /** Re-read one level after a registration or removal, so the tree and the pages agree at once. */
  refreshSchemas: (catalog: string) => Promise<void>;
  refreshTables: (catalog: string, schema: string) => Promise<void>;
}

const TreeContext = createContext<TreeState | null>(null);
export const useCatalogTree = () => {
  const ctx = useContext(TreeContext);
  if (!ctx) throw new Error("useCatalogTree must be used inside CatalogShell");
  return ctx;
};

export const tableKey = (catalog: string, schema: string) => `${catalog} ${schema}`;

/**
 * The disclosure mark, in the same icon vocabulary as every other mark on the
 * page. One glyph that rotates, so open and closed are the same shape in two
 * positions rather than two shapes a reader has to tell apart.
 */
function Twist({ open }: { open: boolean }) {
  return (
    <span className={`${s.chev} ${open ? s.chevOpen : ""}`} aria-hidden="true">
      <i className="fas fa-chevron-right" />
    </span>
  );
}

// Data catalogs first, in their own order; the platform's own catalog last,
// because it is the one nothing in the rail navigates into.
function catalogOrder(source: EngineSource): number {
  return isSystemCatalog(source.catalog) ? 2 : source.catalog === "OpenSource" ? 0 : 1;
}

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
  const { isAdmin } = useRole();
  // KaveonDB's record families. The Engine publishes no `product` schema into
  // its query snapshot — the name is the facade its parser resolves to the
  // record transactions — so the rail reads them from the platform, and only
  // for an administrator, who is the only role the Engine answers for.
  const [systemTables, setSystemTables] = useState<SystemTable[] | null>(null);
  // A refusal and an empty catalog are different facts, and the node says
  // which. A server that does not publish the reading is not a catalog with
  // nothing in it.
  const [systemError, setSystemError] = useState<string | null>(null);

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

  const refreshSchemas = useCallback(async (catalog: string) => {
    const src = sourceFor(catalog);
    if (!src) return;
    try { const list = await fetchSchemas(src.id); setSchemas(prev => ({ ...prev, [catalog]: list })); } catch (e) { fail(e); }
  }, [sourceFor]);

  const refreshTables = useCallback(async (catalog: string, schema: string) => {
    const src = sourceFor(catalog), k = tableKey(catalog, schema);
    if (!src) return;
    try { const list = await fetchTables(src.id, schema); setTables(prev => ({ ...prev, [k]: list })); } catch (e) { fail(e); }
  }, [sourceFor]);

  useEffect(() => {
    let cancelled = false;
    // Warm the durable definition snapshot while the shell resolves its
    // navigation tree. Inventory consumes the shared in-flight request, so
    // opening KaveonDB does not start a second metadata wait.
    void fetchDefinitions()
      .then(definitions => Promise.all(definitions.map(definition => fetchSchemaDefinitions(definition.id))))
      .catch(() => undefined);
    fetchSources()
      .then(list => {
        if (!cancelled) {
          const ordered = [...list].sort((a, b) => catalogOrder(a) - catalogOrder(b) || a.name.localeCompare(b.name));
          setCatalogs(ordered);
          setError(null);
          // Keep the index calm on large installations. Opening every catalog
          // here used to trigger a schema read for every source and made a
          // hundred-catalog tenant feel like one enormous table. A deep link
          // is opened by the path effect below; the overview starts collapsed.
          setOpen(current.catalog ? new Set([current.catalog]) : new Set());
        }
      })
      .catch(e => { if (!cancelled) fail(e); });
    return () => { cancelled = true; };
  }, []);

  // The structure of the system catalog costs nothing to read, so the rail
  // asks for it without counts. A refusal leaves the node listing no tables
  // rather than putting an error over the whole tree.
  useEffect(() => {
    if (!isAdmin) { setSystemTables(null); setSystemError(null); return; }
    let cancelled = false;
    fetchSystemCatalog()
      .then(reading => { if (!cancelled) { setSystemTables(reading.tables); setSystemError(null); } })
      .catch(failure => {
        if (cancelled) return;
        setSystemTables([]);
        setSystemError(failure instanceof Error ? failure.message : "The reading did not arrive.");
      });
    return () => { cancelled = true; };
  }, [isAdmin]);

  // A deep link lands expanded along its own path.
  useEffect(() => {
    if (!current.catalog) return;
    if (!isSystemCatalog(current.catalog)) loadSchemas(current.catalog);
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

  const value = useMemo<TreeState>(() => ({ catalogs, schemas, tables, error, sourceFor, loadSchemas, loadTables, refreshSchemas, refreshTables }),
    [catalogs, schemas, tables, error, sourceFor, loadSchemas, loadTables, refreshSchemas, refreshTables]);

  return (
    <TreeContext.Provider value={value}>
      <div className={`page-shell ${s.root} ${collapsed ? s.rootCollapsed : ""}`}>
        <aside className={s.tree} aria-label="Catalogs">
          {collapsed ? (
            <div className={s.rail}>
              <button type="button" className={s.collapse} onClick={() => setCollapsed(false)} aria-label="Expand catalog tree"><i className="fas fa-angles-right" aria-hidden="true" /></button>
            </div>
          ) : (
            <>
              <div className={s.treeHead}>
                {/* The rail's one job is named once. A label above the label
                    would say "Data catalog" to a reader already inside it. */}
                <h2 className={s.treeTitle}>Catalogs{catalogs && <span>{catalogs.length}</span>}</h2>
                <button type="button" className={s.collapse} onClick={() => setCollapsed(true)} aria-label="Collapse catalog tree"><i className="fas fa-angles-left" aria-hidden="true" /></button>
              </div>
              <div className={s.treeBody}>
                {!catalogs && !error && <div className={s.treeNote}>Loading catalogs…</div>}
                {error && <div className={`${s.treeNote} ${s.treeErr}`}>{error.message}</div>}
                {catalogs && catalogs.length === 0 && <div className={s.treeNote}>No catalogs are registered.</div>}
                {catalogs?.map(cat => {
                  const name = cat.catalog, isOpen = open.has(name), list = schemas[name];
                  // The platform's own catalog is listed, not navigated. Its
                  // record families are not Engine tables a page can open, so
                  // the node states what it holds and goes no further; the
                  // Catalog page reads it in full below the inventory.
                  if (isSystemCatalog(name)) {
                    return (
                      <SystemNode
                        key={name} identifier={name} open={isOpen} isAdmin={isAdmin}
                        tables={systemTables} failure={systemError} onToggle={() => toggle(name)}
                      />
                    );
                  }
                  return (
                    <div key={name}>
                      <button type="button" className={`${s.node} ${s.nodeTop}`} aria-expanded={isOpen} onClick={() => toggle(name, () => loadSchemas(name))}>
                        <Twist open={isOpen} />
                        <span className={s.kind}><i className="fas fa-database" aria-hidden="true" /></span>
                        <span className={s.nodeLabel}>{catalogLabel(name)}</span>
                        <span className={s.nodeCount}>{list ? list.length : ""}</span>
                      </button>
                      {isOpen && (list ?? []).map(schema => {
                        const k = tableKey(name, schema), sOpen = open.has(k), tlist = tables[k];
                        const schemaActive = current.catalog === name && current.schema === schema && !current.table;
                        return (
                          <div key={k}>
                            {/* The twist and the name are two controls, not one: the
                                twist opens and closes the schema where it stands, the
                                name navigates to it. A schema that opened on arrival
                                closes again from the same place it opened. */}
                            <div className={`${s.row} ${s.level1} ${schemaActive ? s.rowActive : ""}`}>
                              <button type="button" className={s.twist} aria-expanded={sOpen}
                                aria-label={`${sOpen ? "Collapse" : "Expand"} ${schema}`}
                                onClick={() => toggle(k, () => loadTables(name, schema))}>
                                <Twist open={sOpen} />
                              </button>
                              <Link href={`/catalog/${enc(name)}/${enc(schema)}`} className={s.rowLink} aria-current={schemaActive ? "page" : undefined}
                                onClick={() => { setOpen(prev => new Set(prev).add(k)); loadTables(name, schema); }}>
                                <span className={s.kind}><i className="fas fa-folder" aria-hidden="true" /></span>
                                <span className={s.nodeLabel}>{schema}</span>
                                <span className={s.nodeCount}>{tlist ? tlist.length : ""}</span>
                              </Link>
                            </div>
                            {sOpen && (tlist ?? []).map(table => {
                              const active = current.catalog === name && current.schema === schema && current.table === table;
                              return (
                                <Link key={table} href={`/catalog/${enc(name)}/${enc(schema)}/${enc(table)}`} className={`${s.node} ${s.level2}`} aria-current={active ? "page" : undefined}>
                                  <span className={s.chev} />
                                  <span className={s.kind}><i className="fas fa-table" aria-hidden="true" /></span>
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
              {/* The rail's one standing action follows the last row it lists
                  rather than the floor of the viewport, so the tree and the
                  thing a reader does with it stay one block. */}
              <div className={s.treeFoot}>
                <Link href={current.catalog ? `/lab?catalog=${enc(current.catalog)}${current.schema ? `&schema=${enc(current.schema)}` : ""}` : "/lab"} className={`${s.ghost} ${s.treeAction}`} aria-current={pathname.startsWith("/catalog/query") ? "page" : undefined}><i className="fas fa-code" aria-hidden="true" /> SQL Lab</Link>
              </div>
            </>
          )}
        </aside>
        <div className={s.pane} key={pathname}>{children}</div>
      </div>
    </TreeContext.Provider>
  );
}

/**
 * KaveonDB in the rail.
 *
 * Every other node here is a way into something: a catalog opens to schemas,
 * a schema is a page, a table is a page. This one holds the platform's own
 * records, which no catalog page can open and no statement can scan, so it
 * opens to a listing and stops. Nothing in it is a link, a twist or a button,
 * which is the marking: a reader finds out it is read-only by there being
 * nothing to press, not by a badge saying so.
 *
 * The name the product uses and the name a statement uses are not the same
 * word, so the node carries both: KaveonDB leads, and the identifier the
 * Engine resolves sits in the slot every other catalog uses for its schema
 * count. It is read from the catalog record rather than written here, so a
 * rename on the Engine arrives without a change in Studio. A reader without
 * the administrator role sees the catalog and one line saying why it lists
 * nothing, rather than an empty node or no node at all.
 */
function SystemNode({ identifier, open, isAdmin, tables, failure, onToggle }: {
  identifier: string; open: boolean; isAdmin: boolean; tables: SystemTable[] | null;
  failure: string | null; onToggle: () => void;
}) {
  return (
    <div>
      <button type="button" className={`${s.node} ${s.nodeTop}`} aria-expanded={open} onClick={onToggle}
        title={`${SYSTEM_CATALOG_LABEL} resolves as ${identifier} in a statement.`}>
        <Twist open={open} />
        <span className={s.kind}><i className="fas fa-database" aria-hidden="true" /></span>
        <span className={s.nodeLabel}>{SYSTEM_CATALOG_LABEL}</span>
        <span className={`${s.nodeCount} ${s.nodeIdent}`}>{identifier}</span>
      </button>
      {open && !isAdmin && (
        <div className={`${s.treeNote} ${s.level1}`}>
          The platform&rsquo;s own records. Browsing its tables requires the administrator role.
        </div>
      )}
      {open && isAdmin && (
        <>
          <div className={`${s.node} ${s.level1} ${s.nodeStatic}`}>
            <span className={s.chev} />
            <span className={s.kind}><i className="fas fa-folder" aria-hidden="true" /></span>
            <span className={s.nodeLabel}>{SYSTEM_CATALOG_SCHEMA}</span>
            <span className={s.nodeCount}>{tables ? tables.length : ""}</span>
          </div>
          {(tables ?? []).map(table => (
            <div key={table.table} className={`${s.node} ${s.level2} ${s.nodeStatic}`} title={table.identifier}>
              <span className={s.chev} />
              <span className={s.kind}><i className="fas fa-table" aria-hidden="true" /></span>
              <span className={s.nodeLabel}>{table.table}</span>
              <span className={s.nodeCount} />
            </div>
          ))}
          {!tables && <div className={`${s.treeNote} ${s.level2}`}>Reading</div>}
          {tables?.length === 0 && (
            <div className={`${s.treeNote} ${s.level2}`}>
              {failure ?? "This catalog holds no record families."}
            </div>
          )}
        </>
      )}
    </div>
  );
}
