"use client";

/**
 * The inventory — every table Kaveon can answer over, and what it knows about
 * each one.
 *
 * The subject of this page is tables. Catalogs and schemas are the address a
 * table lives at, so they appear once, as the label of the section that holds
 * their tables, and nowhere else: the rail on the left is navigation and is
 * not repeated here.
 *
 * Two reads compose one row. The Engine's table definitions arrive first and
 * are cheap — they draw every row with its name, format and columns, and
 * reserve the space the measured cells will occupy. The per-schema inventory
 * follows and fills those cells with what the Engine has actually measured:
 * rows, bytes, files, when the source last changed, and whether the statistics
 * on record still describe it. Nothing moves when the second read lands.
 *
 * Every number here was measured. A table nobody has analyzed shows an em
 * dash, never a zero, and its row says so in words.
 */

import Link from "next/link";
import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useRole } from "../../hooks/useRole";
import s from "./catalog.module.css";
import {
  AnalyzeDepth, CatalogDefinition, CatalogError, EngineTable, Measurement, SchemaDefinition,
  SourceVersion, analyzeTable, bytes, count, enc, exactTime, fetchDefinitions, fetchInventory,
  fetchSchemaDefinitions, fetchTableDefinitions, formatKind, formatLabel, labHref, since, versionDigest,
  versionLabel,
} from "./lib";

type SortKey = "name" | "rows" | "bytes" | "changed";
type Sort = { key: SortKey; desc: boolean };
type Group = { catalogId: string; catalog: string; schemaId: string; schema: string; tables: EngineTable[] | null };
interface Row { group: Group; table: EngineTable; measured: Measurement | undefined }

/** The three forms of ANALYZE, each in the one sentence that says what it costs and what it buys. */
const ANALYZE_FORMS: { label: string; depth: AnalyzeDepth; needsShape?: boolean; blurb: string }[] = [
  {
    label: "Metadata", depth: {},
    blurb: "Reads the Parquet footers, the Delta log or the Iceberg manifests and no data pages, so it finishes in seconds and puts row counts, size and column bounds on record.",
  },
  {
    label: "Sketches", depth: { sketches: true },
    blurb: "Reads every column once to build distinct-count and quantile sketches, so how many distinct values a column holds is answered later without reading the table again.",
  },
  {
    label: "Cube", depth: { cube: true }, needsShape: true,
    blurb: "Builds the cells of the table's declared shape in the same scan, so grouped totals over those dimensions are answered from the cells instead of the rows.",
  },
];

/** What the statistics mean for the reader, in one word and one sentence. */
const STATE = {
  current: {
    word: "Current", tone: s.toneOk,
    says: "These statistics describe the source as it stands now, so counts, totals and column bounds over this table are answered without reading the data.",
  },
  stale: {
    word: "Stale", tone: s.toneWarn,
    says: "The source changed after these statistics were computed, so they no longer describe it and every question about this table is answered by reading it.",
  },
  none: {
    word: "None", tone: s.toneOff,
    says: "This table has never been analyzed, so every question about it is answered by reading it.",
  },
  unreadable: {
    word: "Unreadable", tone: s.toneBad,
    says: "The Engine could not read this location.",
  },
} as const;
type State = typeof STATE[keyof typeof STATE];

function stateOf(measured: Measurement | undefined): State | null {
  if (!measured) return null;
  if (measured.state === "unreadable") return STATE.unreadable;
  if (measured.state === "unmeasured") return STATE.none;
  return measured.stale ? STATE.stale : STATE.current;
}

export function Inventory({ catalogName, schemaName, action }: {
  catalogName?: string; schemaName?: string;
  /** The one thing this list can be missing, offered beside the list itself. */
  action?: React.ReactNode;
}) {
  const { isAdmin, isEditor, isAnalyst } = useRole();
  const [catalogs, setCatalogs] = useState<CatalogDefinition[] | null>(null);
  const [error, setError] = useState<CatalogError | null>(null);
  const [groups, setGroups] = useState<Group[] | null>(null);
  const [measured, setMeasured] = useState<Record<string, Record<string, Measurement>>>({});
  const [measuring, setMeasuring] = useState<Record<string, "loading" | "done" | "failed">>({});
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<Sort>({ key: "name", desc: false });
  const [open, setOpen] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const live = useRef(true);

  useEffect(() => {
    live.current = true;
    return () => { live.current = false; };
  }, []);

  /** Definitions first, then the measurements that fill the cells they drew. */
  const load = useCallback(async (refresh: boolean) => {
    try {
      const definitions = await fetchDefinitions();
      if (!live.current) return;
      setCatalogs(definitions);
      setError(null);
      const listed = await Promise.all(definitions.map(async catalog => {
        const schemas = await fetchSchemaDefinitions(catalog.id).catch(() => [] as SchemaDefinition[]);
        return schemas
          .filter(schema => !schemaName || (catalog.name === catalogName && schema.name === schemaName))
          .map<Group>(schema => ({
            catalogId: catalog.id, catalog: catalog.name,
            schemaId: schema.id, schema: schema.name, tables: null,
          }));
      }));
      const flat = listed.flat();
      if (!live.current) return;
      setGroups(flat);
      await Promise.all(flat.map(async group => {
        const tables = await fetchTableDefinitions(group.schemaId).catch(() => [] as EngineTable[]);
        if (!live.current) return;
        setGroups(current => (current ?? []).map(g => g.schemaId === group.schemaId ? { ...g, tables } : g));
        if (!tables.length) {
          setMeasuring(current => ({ ...current, [group.schemaId]: "done" }));
          return;
        }
        setMeasuring(current => ({ ...current, [group.schemaId]: "loading" }));
        try {
          const list = await fetchInventory(group.schemaId, refresh);
          if (!live.current) return;
          setMeasured(current => ({
            ...current,
            [group.schemaId]: Object.fromEntries(list.map(entry => [entry.tableId, entry])),
          }));
          setMeasuring(current => ({ ...current, [group.schemaId]: "done" }));
        } catch {
          if (live.current) setMeasuring(current => ({ ...current, [group.schemaId]: "failed" }));
        }
      }));
    } catch (e) {
      if (live.current) {
        setError(e instanceof CatalogError ? e : new CatalogError(0, "The catalog could not be read."));
      }
    }
  }, [catalogName, schemaName]);

  useEffect(() => { void load(false); }, [load]);

  const refresh = async () => {
    setRefreshing(true);
    await load(true);
    if (live.current) setRefreshing(false);
  };

  const rows = useMemo<Row[]>(() => (groups ?? []).flatMap(group =>
    (group.tables ?? []).map(table => ({ group, table, measured: measured[group.schemaId]?.[table.id] }))),
    [groups, measured]);

  const term = query.trim().toLowerCase();
  const matching = useMemo(() => !term ? rows : rows.filter(({ group, table }) =>
    table.name.toLowerCase().includes(term)
    || `${group.catalog}.${group.schema}.${table.name}`.toLowerCase().includes(term)
    || table.columns.some(column => column.name.toLowerCase().includes(term))),
    [rows, term]);

  const totals = useMemo(() => {
    const seen = matching.filter(row => row.measured?.state === "measured");
    return {
      shown: matching.length, all: rows.length, measured: seen.length,
      rows: seen.reduce((sum, row) => sum + (row.measured?.rows ?? 0), 0),
      bytes: seen.reduce((sum, row) => sum + (row.measured?.bytes ?? 0), 0),
      schemas: new Set(matching.map(row => row.group.schemaId)).size,
    };
  }, [matching, rows.length]);

  /** Sections keep their address order; only the rows inside them are ranked. */
  const sections = useMemo(() => (groups ?? []).map(group => ({
    group,
    rows: matching.filter(row => row.group.schemaId === group.schemaId).sort(compare(sort)),
  })).filter(section => !term || section.rows.length > 0), [groups, matching, sort, term]);

  const resort = (key: SortKey) =>
    setSort(current => ({ key, desc: current.key === key ? !current.desc : key !== "name" }));

  if (error) {
    return (
      <div className={`${s.note} ${s.noteErr}`} role="alert">
        <b>The catalog is unavailable.</b> {error.message}
      </div>
    );
  }

  if (catalogs && catalogs.length === 0) {
    return (
      <div className={s.empty}>
        <h1 className={s.emptyTitle}>{isAdmin ? "No catalogs registered" : "No catalogs available to you"}</h1>
        <p className={s.emptyBody}>
          {isAdmin
            ? "A catalog is a storage location Kaveon reads in place — a container in ADLS Gen2, a bucket in S3, or a directory on the coordinator. Register one and the schemas and tables under it appear here and in SQL Lab."
            : "Catalogs are registered and granted by an administrator. Ask for access to one and its tables appear here and in SQL Lab."}
        </p>
        {isAdmin && (
          <div className={s.emptyActions}>
            <Link href="/settings/storage" className={`${s.ghost} ${s.primary}`}>Register a catalog</Link>
          </div>
        )}
      </div>
    );
  }

  const reading = !groups || sections.some(section => section.group.tables === null);

  return (
    <>
      <header className={s.invHead}>
        <div className={s.invHeadTop}>
          <h1 className={s.invTitle}>{schemaName ?? "Tables"}</h1>
          <div className={s.invActions}>
            <label className={s.search}>
              <i className="fas fa-magnifying-glass" aria-hidden="true" />
              <input
                type="search" value={query} placeholder="Search tables and columns"
                aria-label="Search tables and columns"
                onChange={event => setQuery(event.target.value)}
              />
            </label>
            {isAdmin && (
              <Link href="/settings/storage" className={s.ghost}>
                <i className="fas fa-sliders" aria-hidden="true" /> Catalog sources
              </Link>
            )}
            <button type="button" className={s.ghost} onClick={refresh} disabled={refreshing}>
              <i className={`fas fa-rotate ${refreshing ? s.rotate : ""}`} aria-hidden="true" />
              {refreshing ? "Measuring" : "Re-measure"}
            </button>
            {action}
          </div>
        </div>
        <p className={s.invLede}>
          A table whose statistics are current can answer counts, totals and bounds without reading the data.
          One whose statistics are stale, or missing, is read in full for every question asked of it.
        </p>
      </header>

      <p className={s.invSummary} aria-live="polite">
        {reading ? "Reading the catalog" : summary(totals, !!term, !schemaName)}
      </p>

      {!reading && sections.length === 0 && (
        <div className={s.empty}>
          <h2 className={s.emptyTitle}>{term ? "Nothing matches that" : "No schemas yet"}</h2>
          <p className={s.emptyBody}>
            {term
              ? "Search runs over table names and column names. Clear it to see every table again."
              : "A schema groups the tables inside a catalog. Add one, then register the Delta, Iceberg or Parquet tables under it; each is read once to verify it before it joins the catalog."}
          </p>
        </div>
      )}

      {sections.length > 0 && (
        <div className={s.tableWrap}>
          <table className={s.inv}>
            <thead>
              <tr>
                <th className={s.cDisclose}><span className={s.sr}>Details</span></th>
                <SortHeader label="Table" sortKey="name" sort={sort} onSort={resort} className={s.cName} />
                <th className={s.cFormat}>Format</th>
                <SortHeader label="Rows" sortKey="rows" sort={sort} onSort={resort} className={s.cRows} />
                <SortHeader label="Size" sortKey="bytes" sort={sort} onSort={resort} className={s.cSize} />
                <SortHeader label="Changed" sortKey="changed" sort={sort} onSort={resort} className={s.cWhen} />
                <th className={s.cState}>Statistics</th>
              </tr>
            </thead>
            {sections.map(({ group, rows: sectionRows }) => (
              <tbody key={group.schemaId} className={s.schemaBody}>
                {/* Scoped to one schema, the heading above already names it;
                    a band repeating it would be the third place it appears. */}
                {!schemaName && (
                  <tr className={s.groupRow}>
                    <th colSpan={7} scope="colgroup">
                      <div className={s.groupBand}>
                        <span className={s.groupName}>
                          <span className={s.groupCatalog}>{group.catalog}.</span>{group.schema}
                        </span>
                        <span className={s.groupCount}>
                          {group.tables === null ? "reading"
                            : `${group.tables.length} table${group.tables.length === 1 ? "" : "s"}`}
                        </span>
                      </div>
                    </th>
                  </tr>
                )}
                {group.tables === null && [0, 1, 2].map(index => <SkeletonRow key={index} />)}
                {group.tables !== null && group.tables.length === 0 && (
                  <tr className={s.invRow}>
                    <td className={s.cDisclose} />
                    <td colSpan={6} className={s.groupEmpty}>
                      No tables are registered in this schema.
                      {isEditor ? " Register one by its location in the catalog's storage; the Engine reads it once to verify it." : ""}
                    </td>
                  </tr>
                )}
                {sectionRows.map(row => (
                  <TableRow
                    key={row.table.id} row={row}
                    pending={measuring[row.group.schemaId] === "loading" && !row.measured}
                    expanded={open === row.table.id}
                    onToggle={() => setOpen(current => current === row.table.id ? null : row.table.id)}
                    isEditor={isEditor} isAnalyst={isAnalyst}
                    onMeasured={entry => setMeasured(current => ({
                      ...current,
                      [row.group.schemaId]: { ...(current[row.group.schemaId] ?? {}), [row.table.id]: entry },
                    }))}
                  />
                ))}
              </tbody>
            ))}
          </table>
        </div>
      )}
    </>
  );
}

function summary(totals: { shown: number; all: number; measured: number; rows: number; bytes: number; schemas: number },
                 filtered: boolean, acrossSchemas: boolean): string {
  const unmeasured = totals.shown - totals.measured;
  const head = filtered
    ? `${count(totals.shown)} of ${count(totals.all)} tables`
    : `${count(totals.shown)} table${totals.shown === 1 ? "" : "s"}`;
  const where = acrossSchemas
    ? ` in ${count(totals.schemas)} schema${totals.schemas === 1 ? "" : "s"}.`
    : ".";
  const seen = totals.measured > 0
    ? ` ${count(totals.measured)} measured: ${count(totals.rows)} rows over ${bytes(totals.bytes)}.`
    : " None measured yet.";
  return head + where + seen + (unmeasured > 0 ? ` ${count(unmeasured)} not yet measured.` : "");
}

function SortHeader({ label, sortKey, sort, onSort, className }: {
  label: string; sortKey: SortKey; className: string; sort: Sort; onSort: (key: SortKey) => void;
}) {
  const active = sort.key === sortKey;
  return (
    <th className={className} aria-sort={active ? (sort.desc ? "descending" : "ascending") : "none"}>
      <button type="button" className={`${s.sortBtn} ${active ? s.sortOn : ""}`} onClick={() => onSort(sortKey)}>
        {label}
        <i className={`fas fa-caret-${active && sort.desc ? "down" : "up"}`} aria-hidden="true" />
      </button>
    </th>
  );
}

function compare(sort: Sort) {
  const direction = sort.desc ? -1 : 1;
  return (a: Row, b: Row) => {
    if (sort.key === "name") return a.table.name.localeCompare(b.table.name) * direction;
    const pick = (row: Row) => sort.key === "rows" ? row.measured?.rows
      : sort.key === "bytes" ? row.measured?.bytes : row.measured?.lastModifiedMs;
    const left = pick(a), right = pick(b);
    // A table nobody has measured has no place on a ranking of measurements:
    // it sorts to the end either way rather than posing as a zero.
    if (typeof left !== "number" && typeof right !== "number") return a.table.name.localeCompare(b.table.name);
    if (typeof left !== "number") return 1;
    if (typeof right !== "number") return -1;
    return (left - right) * direction;
  };
}

function SkeletonRow() {
  return (
    <tr className={s.invRow} aria-hidden="true">
      <td className={s.cDisclose} />
      <td className={s.cName}><span className={s.skel} style={{ width: 132 }} /></td>
      <td className={s.cFormat}><span className={s.skel} style={{ width: 58 }} /></td>
      <td className={s.cRows}><span className={s.skel} style={{ width: 52 }} /></td>
      <td className={s.cSize}><span className={s.skel} style={{ width: 44 }} /></td>
      <td className={s.cWhen}><span className={s.skel} style={{ width: 54 }} /></td>
      <td className={s.cState}><span className={s.skel} style={{ width: 58 }} /></td>
    </tr>
  );
}

function TableRow({ row, pending, expanded, onToggle, isEditor, isAnalyst, onMeasured }: {
  row: Row; pending: boolean; expanded: boolean; onToggle: () => void;
  isEditor: boolean; isAnalyst: boolean; onMeasured: (entry: Measurement) => void;
}) {
  const { group, table, measured } = row;
  const state = stateOf(measured);
  const version = measured?.currentSourceVersion ?? measured?.sourceVersion ?? null;
  const href = `/catalog/${enc(group.catalog)}/${enc(group.schema)}/${enc(table.name)}`;
  const waiting = (width: number) => pending
    ? <span className={s.skel} style={{ width }} />
    : <span className={s.dash}>—</span>;
  const shape = table.shape?.dimensions?.length ? table.shape : null;
  const partitions = measured?.partitionColumns?.length
    ? measured.partitionColumns
    : (table.partitions ?? []).map(partition => partition.name);
  const clustered = table.layout?.clustered_by ?? [];
  const files = measured?.state === "measured" ? measured.files ?? null
    : version?.kind === "listing" ? version.files
    : version?.kind === "file" ? 1 : null;

  return (
    <>
      <tr className={`${s.invRow} ${expanded ? s.invRowOpen : ""} ${state ? state.tone : ""}`}>
        <td className={s.cDisclose}>
          <button
            type="button" className={s.disclose} onClick={onToggle}
            aria-expanded={expanded} aria-controls={`d-${table.id}`}
            aria-label={`Details for ${table.name}`}
          >
            <i className={`fas fa-chevron-${expanded ? "down" : "right"}`} aria-hidden="true" />
          </button>
        </td>
        <td className={s.cName}>
          <span className={s.nameLine}>
            <Link href={href} className={s.tableName}>{table.name}</Link>
            {partitions.length > 0 && <span className={s.mark} title={`Partitioned by ${partitions.join(", ")}`}>partitioned</span>}
            {clustered.length > 0 && <span className={s.mark} title={`Clustered by ${clustered.join(", ")}`}>clustered</span>}
            {shape && <span className={s.mark} title="A cube shape is declared over this table">shaped</span>}
          </span>
          <span className={s.narrowFacts} aria-hidden="true">
            <span>{[formatLabel(table.format), formatKind(table.format, version)].filter(Boolean).join(" ")}</span>
            {measured?.state === "measured" && <span>{count(measured.rows)} rows</span>}
            {measured?.state === "measured" && <span>{bytes(measured.bytes)}</span>}
          </span>
        </td>
        <td className={s.cFormat}>
          <span className={s.formatCell}>
            <span>{formatLabel(table.format)}</span>
            {formatKind(table.format, version) && <span className={s.formatKind}>{formatKind(table.format, version)}</span>}
          </span>
        </td>
        <td className={s.cRows}>{measured?.state === "measured" ? count(measured.rows) : waiting(52)}</td>
        <td className={s.cSize}>
          <span className={s.sizeCell}>
            <span>{measured?.state === "measured" ? bytes(measured.bytes) : waiting(44)}</span>
            {typeof files === "number"
              ? <span className={s.sizeFiles}>{count(files)} file{files === 1 ? "" : "s"}</span>
              : <span className={s.sizeFiles}>{pending ? <span className={s.skel} style={{ width: 30 }} /> : ""}</span>}
          </span>
        </td>
        <td className={s.cWhen} title={exactTime(measured?.lastModifiedMs)}>
          {typeof measured?.lastModifiedMs === "number" ? since(measured.lastModifiedMs) : waiting(54)}
        </td>
        <td className={s.cState}>
          {state ? (
            <span className={s.stateCell} title={state.says}>
              <span className={s.stateWord}>{state.word}</span>
              {measured?.state === "measured" && (
                <span className={s.stateDepth}>{measured.depth === "full" ? "full, with sketches" : "metadata only"}</span>
              )}
            </span>
          ) : waiting(58)}
        </td>
      </tr>
      {expanded && (
        <tr className={s.detailRow}>
          <td colSpan={7} id={`d-${table.id}`}>
            <Detail
              row={row} state={state} version={version} shape={shape}
              partitions={partitions} clustered={clustered}
              isEditor={isEditor} isAnalyst={isAnalyst} href={href} onMeasured={onMeasured}
            />
          </td>
        </tr>
      )}
    </>
  );
}

function Detail({ row, state, version, shape, partitions, clustered, isEditor, isAnalyst, href, onMeasured }: {
  row: Row; state: State | null; version: SourceVersion | null; shape: EngineTable["shape"] | null;
  partitions: string[]; clustered: string[];
  isEditor: boolean; isAnalyst: boolean; href: string; onMeasured: (entry: Measurement) => void;
}) {
  const { group, table, measured } = row;
  const [running, setRunning] = useState<string | null>(null);
  const [outcome, setOutcome] = useState<{ ok: boolean; text: string } | null>(null);

  const run = async (label: string, depth: AnalyzeDepth) => {
    setRunning(label);
    setOutcome(null);
    try {
      const done = await analyzeTable(table.id, depth);
      const cells = done.result.cube_cells;
      setOutcome({
        ok: true,
        text: `${done.statement} — ${count(done.result.row_count ?? null)} rows${typeof cells === "number" ? `, ${count(cells)} cube cells` : ""}.`,
      });
      const fresh = await fetchInventory(group.schemaId, true);
      const entry = fresh.find(item => item.tableId === table.id);
      if (entry) onMeasured(entry);
    } catch (e) {
      setOutcome({ ok: false, text: e instanceof Error ? e.message : "The table could not be measured." });
    } finally {
      setRunning(null);
    }
  };

  return (
    <div className={s.detail}>
      <dl className={s.facts}>
        <dt>Location</dt>
        <dd className={s.mono}>{table.location}</dd>

        <dt>Source version</dt>
        <dd>
          <span className={s.mono}>{versionLabel(version)}</span>
          {versionDigest(version) && <span className={s.digest}>{versionDigest(version)}</span>}
          {typeof measured?.observedAtMs === "number" && (
            <span className={s.aside} title={exactTime(measured.observedAtMs)}>read {since(measured.observedAtMs)}</span>
          )}
        </dd>

        <dt>Statistics</dt>
        <dd>
          {state?.says}
          {measured?.state === "measured" && (
            <span className={s.aside} title={exactTime(measured.computedAtMs)}>
              {`Measured ${since(measured.computedAtMs)}`}
              {measured.depth === "full"
                ? ", every column read and the distinct-count and quantile sketches kept"
                : ", from the source's own metadata with no data pages read"}
              {typeof measured.rowGroups === "number" ? `, ${count(measured.rowGroups)} row groups` : ""}
              {typeof measured.uncompressedBytes === "number" ? `, ${bytes(measured.uncompressedBytes)} uncompressed` : ""}
              {typeof measured.measuredColumns === "number" ? `, ${count(measured.measuredColumns)} columns described` : ""}.
            </span>
          )}
        </dd>

        {measured?.state === "unreadable" && (
          <>
            <dt className={s.factBad}>Storage</dt>
            <dd className={`${s.mono} ${s.factBad}`}>{measured.error}</dd>
          </>
        )}

        {partitions.length > 0 && (
          <>
            <dt>Partitioned by</dt>
            <dd className={s.mono}>{partitions.join(", ")}</dd>
          </>
        )}
        {clustered.length > 0 && (
          <>
            <dt>Clustered by</dt>
            <dd className={s.mono}>{clustered.join(", ")}</dd>
          </>
        )}
        {shape && (
          <>
            <dt>Declared shape</dt>
            <dd>
              <span className={s.mono}>{shape.dimensions?.length ?? 0} dimensions, {shape.measures?.length ?? 0} measures</span>
              <span className={s.aside}>A cube built over this shape answers grouped totals from its cells.</span>
            </dd>
          </>
        )}

        <dt>Columns</dt>
        <dd>
          {count(table.columns.length)} declared
          {table.access ? `, read as ${table.access.toLowerCase()}` : ""}
        </dd>
      </dl>

      <div className={s.detailActions}>
        <Link href={href} className={s.ghost}>Open table</Link>
        <Link href={labHref(group.catalog, group.schema, table.name)} className={s.ghost}>Query in SQL Lab</Link>
        {isAnalyst && <Link href="/datasets/new" className={s.ghost}>Create dataset</Link>}
      </div>

      {isEditor && (
        <div className={s.analyze}>
          <h3 className={s.analyzeTitle}>Measure this table</h3>
          <ul className={s.forms}>
            {ANALYZE_FORMS.map(form => {
              const blocked = form.needsShape && !shape;
              return (
                <li key={form.label}>
                  <button
                    type="button" className={s.formBtn} disabled={running !== null || blocked}
                    onClick={() => run(form.label, form.depth)}
                  >
                    {running === form.label ? "Measuring" : form.label}
                  </button>
                  <p className={s.formBlurb}>
                    {form.blurb}
                    {blocked && " This table declares no shape, so there is nothing for a cube to be built over."}
                  </p>
                </li>
              );
            })}
          </ul>
          {outcome && <p className={outcome.ok ? s.outcomeOk : s.outcomeBad} role="status">{outcome.text}</p>}
        </div>
      )}
    </div>
  );
}
