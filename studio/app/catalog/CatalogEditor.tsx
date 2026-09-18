"use client";

import Link from "next/link";
import { useEffect, useId, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useCatalogTree } from "./CatalogShell";
import s from "./catalog.module.css";
import {
  COLUMN_TYPES, CatalogDefinition, CatalogError, EngineTable, Probe, SchemaDefinition, TableFormat,
  TableUnreadableError, createSchema, createTable, enc, fetchDefinitions, fetchSchemaDefinitions, parseColumns, storageRoot,
} from "./lib";

// Registering a schema or a table is one sheet from the right edge. The user
// works in names; the Engine's definition ids are resolved here from the
// catalog definitions the API lists. A table is verified on KaveonDB before it
// is added: the sheet shows the count and the elapsed time, or the storage
// error the Engine returned, and never leaves a broken table behind.

export type SheetKind = "schema" | "table";
const IDENT = /^[A-Za-z_][A-Za-z0-9_]*$/;
const FORMATS: { value: TableFormat; label: string; hint: string }[] = [
  { value: "Delta", label: "Delta", hint: "A Delta table directory: the snapshot at the latest version is read from its _delta_log." },
  { value: "Iceberg", label: "Iceberg", hint: "An Iceberg table directory: the current snapshot is read from its metadata JSON." },
  { value: "Parquet", label: "Parquet", hint: "One Parquet file, or a directory of Parquet files with one schema (the Hive layout)." },
];
const fmt = (n: number) => n.toLocaleString();

type Phase =
  | { phase: "idle" }
  | { phase: "busy" }
  | { phase: "schema-ok"; schema: SchemaDefinition }
  | { phase: "table-ok"; table: EngineTable; probe: Probe | null }
  | { phase: "failed"; message: string; unreadable: boolean; removed: boolean };

interface SheetProps {
  kind: SheetKind;
  catalog?: string;
  schema?: string;
  onClose: () => void;
}

export function RegisterSheet({ kind: initialKind, catalog: initialCatalog, schema: initialSchema, onClose }: SheetProps) {
  const { catalogs, refreshSchemas, refreshTables } = useCatalogTree();
  const titleId = useId();
  const nameField = useRef<HTMLInputElement>(null);

  const [kind, setKind] = useState<SheetKind>(initialKind);
  const [definitions, setDefinitions] = useState<CatalogDefinition[] | null>(null);
  const [definitionsError, setDefinitionsError] = useState<string | null>(null);
  const [catalog, setCatalog] = useState(initialCatalog ?? "");
  const [schemaDefs, setSchemaDefs] = useState<SchemaDefinition[] | null>(null);
  const [schema, setSchema] = useState(initialSchema ?? "");
  const [name, setName] = useState("");
  const [location, setLocation] = useState("");
  const [format, setFormat] = useState<TableFormat>("Delta");
  const [columnsText, setColumnsText] = useState("");
  const [touched, setTouched] = useState(false);
  const [state, setState] = useState<Phase>({ phase: "idle" });

  const catalogNames = useMemo(() => (catalogs ?? []).map(c => c.catalog), [catalogs]);
  useEffect(() => { if (!catalog && catalogNames.length) setCatalog(catalogNames[0]); }, [catalog, catalogNames]);

  useEffect(() => {
    let cancelled = false;
    fetchDefinitions()
      .then(list => { if (!cancelled) { setDefinitions(list); setDefinitionsError(null); } })
      .catch(e => { if (!cancelled) setDefinitionsError(e instanceof CatalogError ? e.message : "The catalog definitions could not be read."); });
    return () => { cancelled = true; };
  }, []);

  const definition = useMemo(() => definitions?.find(d => d.name === catalog) ?? null, [definitions, catalog]);
  const root = storageRoot(definition);

  useEffect(() => {
    if (!definition || kind !== "table") { setSchemaDefs(null); return; }
    let cancelled = false;
    setSchemaDefs(null);
    fetchSchemaDefinitions(definition.id)
      .then(list => { if (!cancelled) setSchemaDefs(list); })
      .catch(() => { if (!cancelled) setSchemaDefs([]); });
    return () => { cancelled = true; };
  }, [definition, kind]);

  const activeSchemas = useMemo(() => (schemaDefs ?? []).filter(d => d.lifecycle === "Active"), [schemaDefs]);
  useEffect(() => {
    if (kind === "table" && schemaDefs && !activeSchemas.some(d => d.name === schema)) setSchema(activeSchemas[0]?.name ?? "");
  }, [kind, schemaDefs, activeSchemas, schema]);
  const schemaDef = activeSchemas.find(d => d.name === schema) ?? null;

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape" && state.phase !== "busy") onClose(); };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose, state.phase]);
  useEffect(() => { nameField.current?.focus(); }, [kind]);

  const parsed = useMemo(() => parseColumns(columnsText), [columnsText]);
  const nameError = !name ? "A name is required." : !IDENT.test(name) ? "Use letters, digits and underscores, starting with a letter." : null;
  const locationError = kind === "table" && !location.trim() ? "A location is required."
    : /:\/\//.test(location) ? "Write the path relative to the catalog root, not a URI." : null;
  const columnsError = kind === "table" ? (parsed.error ?? (parsed.columns.length ? null : "At least one column is required.")) : null;
  const catalogNote = definitionsError ? definitionsError
    : definitions && !definition && catalog ? `${catalog} is registered on the platform but not yet synchronized with KaveonDB. Synchronize it under Settings → Storage first.`
    : definition && definition.lifecycle !== "Active" ? `${catalog} is ${definition.lifecycle.toLowerCase()} on KaveonDB; activate it before adding to it.`
    : kind === "table" && schemaDefs && activeSchemas.length === 0 ? "This catalog has no active schema yet. Add a schema first."
    : null;
  const ready = !catalogNote && !!definition && !nameError && !locationError && !columnsError && (kind === "schema" || !!schemaDef);
  const busy = state.phase === "busy";

  const submit = async () => {
    setTouched(true);
    if (!ready || !definition) return;
    setState({ phase: "busy" });
    try {
      if (kind === "schema") {
        const created = await createSchema(definition.id, name);
        await refreshSchemas(catalog);
        setState({ phase: "schema-ok", schema: created });
        return;
      }
      const result = await createTable({ schemaId: schemaDef!.id, name, location: location.trim(), format, columns: parsed.columns });
      await refreshTables(catalog, schema);
      setState({ phase: "table-ok", table: result.table, probe: result.probe });
    } catch (e) {
      if (e instanceof TableUnreadableError) setState({ phase: "failed", message: e.message, unreadable: true, removed: e.removed });
      else setState({ phase: "failed", message: e instanceof CatalogError ? e.message : "The request did not complete.", unreadable: false, removed: false });
    }
  };

  const another = () => { setName(""); setLocation(""); setColumnsText(""); setTouched(false); setState({ phase: "idle" }); };
  const addTableToNewSchema = (created: SchemaDefinition) => {
    setKind("table"); setSchema(created.name); setName(""); setTouched(false); setState({ phase: "idle" });
  };

  const done = state.phase === "schema-ok" || state.phase === "table-ok";
  const fullName = `${catalog}.${schema}.${name}`;
  const err = (message: string | null) => touched && message ? <div className={s.fieldErr}>{message}</div> : null;

  const body = (
    <>
      <div className={s.sheetBackdrop} onMouseDown={() => { if (!busy) onClose(); }} />
      <aside className={s.sheet} role="dialog" aria-modal="true" aria-labelledby={titleId}>
        <header className={s.sheetHead}>
          <div style={{ minWidth: 0 }}>
            <div className={s.sheetEyebrow}>{kind === "table" && schema ? `${catalog}.${schema}` : catalog || "Catalog"}</div>
            <h2 className={s.sheetTitle} id={titleId}>{kind === "schema" ? "Add schema" : "Add table"}</h2>
          </div>
          <button type="button" className={s.collapse} onClick={onClose} aria-label="Close" disabled={busy}><i className="fas fa-xmark" /></button>
        </header>

        <div className={s.sheetBody}>
          {state.phase === "schema-ok" && (
            <div className={`${s.result} ${s.resultOk}`} role="status">
              <div className={s.resultHead}><i className="fas fa-check" aria-hidden="true" /> Schema added</div>
              <div className={s.resultNote}><code style={{ fontFamily: "var(--mono)" }}>{catalog}.{state.schema.name}</code> is active on KaveonDB at revision {state.schema.revision}. It appears in the catalog tree and in SQL Lab.</div>
              <div className={s.resultActions}>
                <button type="button" className={`${s.ghost} ${s.primary}`} onClick={() => addTableToNewSchema(state.schema)}><i className="fas fa-plus" aria-hidden="true" /> Add a table to it</button>
                <Link href={`/catalog/${enc(catalog)}/${enc(state.schema.name)}`} className={s.ghost} onClick={onClose}>Open schema</Link>
              </div>
            </div>
          )}
          {state.phase === "table-ok" && (
            <div className={`${s.result} ${s.resultOk}`} role="status">
              <div className={s.resultHead}><i className="fas fa-check" aria-hidden="true" /> {state.probe ? "Verified and added" : "Added"}</div>
              {state.probe && (
                <div className={s.resultFacts}>
                  <span className={s.resultStat}>{fmt(state.probe.rowCount)}<small>rows</small></span>
                  {state.probe.elapsedMs != null && <span className={s.resultStat}>{state.probe.elapsedMs < 1000 ? `${state.probe.elapsedMs} ms` : `${(state.probe.elapsedMs / 1000).toFixed(2)} s`}<small>on KaveonDB</small></span>}
                </div>
              )}
              <div className={s.resultNote}><code style={{ fontFamily: "var(--mono)" }}>{catalog}.{schema}.{state.table.name}</code> is active at revision {state.table.revision}, read in place from <code style={{ fontFamily: "var(--mono)" }}>{root ? `${root}/` : ""}{state.table.location}</code>.</div>
              <div className={s.resultActions}>
                <Link href={`/catalog/${enc(catalog)}/${enc(schema)}/${enc(state.table.name)}`} className={`${s.ghost} ${s.primary}`} onClick={onClose}>Open table</Link>
                <button type="button" className={s.ghost} onClick={another}><i className="fas fa-plus" aria-hidden="true" /> Add another</button>
              </div>
            </div>
          )}
          {state.phase === "failed" && (
            <div className={`${s.result} ${s.resultErr}`} role="alert">
              <div className={s.resultHead}><i className="fas fa-circle-exclamation" aria-hidden="true" /> {state.unreadable ? "KaveonDB could not read the table" : kind === "schema" ? "The schema was not added" : "The table was not added"}</div>
              <pre className={s.resultMessage}>{state.message}</pre>
              <div className={s.resultNote}>
                {state.unreadable
                  ? (state.removed ? "The definition was removed again; nothing was left in the catalog. Check the location and the format, then verify again."
                    : "The definition could not be removed after the failed read. Remove it from the table page before trying again.")
                  : "Nothing was changed."}
              </div>
            </div>
          )}

          {!done && (
            <>
              <div className={s.field}>
                <label className={s.label} htmlFor={`${titleId}-catalog`}>Catalog</label>
                <select id={`${titleId}-catalog`} className={`${s.select} ${s.mono}`} value={catalog} onChange={e => { setCatalog(e.target.value); setSchema(""); }} disabled={busy || !!initialCatalog}>
                  {catalogNames.map(c => <option key={c} value={c}>{c}</option>)}
                  {!catalogNames.length && <option value="">No catalogs</option>}
                </select>
                {catalogNote && <div className={s.fieldErr}>{catalogNote}</div>}
                {!catalogNote && root && <div className={s.hint}>Locations are relative to <code>{root}/</code>.</div>}
              </div>

              {kind === "table" && (
                <div className={s.field}>
                  <label className={s.label} htmlFor={`${titleId}-schema`}>Schema</label>
                  <select id={`${titleId}-schema`} className={`${s.select} ${s.mono}`} value={schema} onChange={e => setSchema(e.target.value)} disabled={busy || !!initialSchema || !schemaDefs}>
                    {!schemaDefs && <option value="">Reading schemas…</option>}
                    {activeSchemas.map(d => <option key={d.id} value={d.name}>{d.name}</option>)}
                  </select>
                </div>
              )}

              <div className={s.field}>
                <label className={s.label} htmlFor={`${titleId}-name`}>{kind === "schema" ? "Schema name" : "Table name"}</label>
                <input id={`${titleId}-name`} className={`${s.input} ${s.mono}`} value={name} onChange={e => setName(e.target.value)} placeholder={kind === "schema" ? "silver" : "orders"} spellCheck={false} autoComplete="off" disabled={busy} ref={nameField} />
                {err(nameError)}
              </div>

              {kind === "table" && (
                <>
                  <div className={s.field}>
                    <label className={s.label} htmlFor={`${titleId}-location`}>Location <small>relative to the catalog root</small></label>
                    <div className={s.prefix}>
                      {root && <span title={root}>{root}/</span>}
                      <input id={`${titleId}-location`} className={`${s.input} ${s.mono}`} value={location} onChange={e => setLocation(e.target.value)} placeholder={format === "Parquet" ? "sales/orders.parquet or sales/orders/" : "sales/orders"} spellCheck={false} autoComplete="off" disabled={busy} />
                    </div>
                    {err(locationError)}
                  </div>

                  <div className={s.field}>
                    <span className={s.label} id={`${titleId}-format`}>Format</span>
                    <div className={s.segment} role="group" aria-labelledby={`${titleId}-format`}>
                      {FORMATS.map(f => <button key={f.value} type="button" aria-pressed={format === f.value} onClick={() => setFormat(f.value)} disabled={busy}>{f.label}</button>)}
                    </div>
                    <div className={s.hint}>{FORMATS.find(f => f.value === format)!.hint}</div>
                  </div>

                  <div className={s.field}>
                    <label className={s.label} htmlFor={`${titleId}-columns`}>Columns <small>one per line</small></label>
                    <textarea id={`${titleId}-columns`} className={`${s.textarea} ${s.mono}`} value={columnsText} onChange={e => setColumnsText(e.target.value)} placeholder={"order_id bigint not null\ncustomer_id bigint\norder_date date\ntotal decimal(18, 2)\nstatus varchar"} spellCheck={false} disabled={busy} />
                    {err(columnsError)}
                    <div className={s.hint}>Name, then type, then <code>not null</code> where a column never holds nulls. Types: {COLUMN_TYPES}. The columns declare the schema KaveonDB reads the table with; every file at the location must carry them in this order.</div>
                  </div>
                </>
              )}
            </>
          )}
        </div>

        <footer className={s.sheetFoot}>
          {busy ? <span className={s.busy}><span className={s.spin} aria-hidden="true" />{kind === "schema" ? "Adding the schema…" : "Reading the table on KaveonDB…"}</span>
            : !done && kind === "table" ? <span className={s.sheetFootNote}>The table is read once before it is added.</span>
            : <span />}
          <div className={s.sheetFootActions}>
            {done ? (
              <button type="button" className={s.ghost} onClick={onClose}>Close</button>
            ) : (
              <>
                <button type="button" className={s.ghost} onClick={onClose} disabled={busy}>Cancel</button>
                <button type="button" className={`${s.ghost} ${s.primary}`} onClick={submit} disabled={busy || (touched && !ready)}>
                  {kind === "schema" ? "Add schema" : state.phase === "failed" ? "Verify again" : "Verify and add"}
                </button>
              </>
            )}
          </div>
        </footer>
      </aside>
    </>
  );

  return typeof document === "undefined" ? null : createPortal(body, document.body);
}

interface RemoveProps {
  fullName: string;
  busy: boolean;
  error: string | null;
  onCancel: () => void;
  onConfirm: () => void;
}

/** Removing a table takes its definition out of the catalog; the data at its location is not touched. */
export function RemoveTableDialog({ fullName, busy, error, onCancel, onConfirm }: RemoveProps) {
  const titleId = useId();
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape" && !busy) onCancel(); };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onCancel, busy]);
  if (typeof document === "undefined") return null;
  return createPortal(
    <div className={s.confirmBackdrop} onMouseDown={e => { if (e.target === e.currentTarget && !busy) onCancel(); }}>
      <div className={s.confirm} role="alertdialog" aria-modal="true" aria-labelledby={titleId}>
        <h2 className={s.confirmTitle} id={titleId}>Remove this table from the catalog?</h2>
        <p className={s.confirmBody}>
          <code>{fullName}</code> stops being queryable on KaveonDB and disappears from SQL Lab, and datasets built on it stop refreshing. The files at its location are not changed; the table can be registered again.
        </p>
        {error && <div className={s.fieldErr} style={{ marginTop: 10 }} role="alert">{error}</div>}
        <div className={s.confirmActions}>
          <button type="button" className={s.ghost} onClick={onCancel} disabled={busy}>Cancel</button>
          <button type="button" className={`${s.ghost} ${s.dangerSolid}`} onClick={onConfirm} disabled={busy}>{busy ? "Removing…" : "Remove table"}</button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
