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
 * The page opens on the state of the lake rather than an explanation of it:
 * how many tables, how many rows, how much storage, and how much of it is
 * accelerated. A row says something in its last cell only when it departs
 * from that baseline — a table carrying sketches, or one that is stale,
 * unmeasured or unreadable. Sixteen tables all saying "current" would be the
 * loudest thing on the page for the least interesting fact.
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

/**
 * Where a table stands against the baseline every measured table shares:
 * statistics on record that describe the source as it is, so counts, totals
 * and column bounds are answered without reading the data. `baseline` is that
 * baseline and says nothing in the row; the other four depart from it and do.
 */
const STANDING = {
  sketches: {
    word: "Sketches", tone: s.standSketch,
    says: "Every column was read and the distinct-count and quantile sketches kept, so distinct counts and quantiles over this table are answered from the sketches instead of the rows.",
  },
  baseline: {
    word: "", tone: "",
    says: "The statistics on record describe the source as it stands, so counts, totals and column bounds over this table are answered without reading the data.",
  },
  stale: {
    word: "Stale", tone: s.standStale,
    says: "The source changed after these statistics were computed, so they no longer describe it and every question about this table is answered by reading it.",
  },
  unmeasured: {
    word: "Not measured", tone: s.standNone,
    says: "This table has never been analyzed, so every question about it is answered by reading it.",
  },
  unreadable: {
    word: "Unreadable", tone: s.standBad,
    says: "The Engine could not read this location.",
  },
} as const;
type Standing = keyof typeof STANDING;

function standingOf(measured: Measurement | undefined): Standing | null {
  if (!measured) return null;
  if (measured.state === "unreadable") return "unreadable";
  if (measured.state === "unmeasured") return "unmeasured";
  if (measured.stale) return "stale";
  return measured.depth === "full" ? "sketches" : "baseline";
}

/** The three standings a reader has to act on, and the only ones worth filtering to. */
const needsAttention = (standing: Standing | null) =>
  standing === "stale" || standing === "unmeasured" || standing === "unreadable";

/** A share of a whole, to one decimal, without rounding a part into the whole or out of existence. */
function share(part: number, whole: number): string {
  if (!whole) return "—";
  if (part === whole) return "100%";
  const pct = (part / whole) * 100;
  if (pct > 0 && pct < 0.1) return "<0.1%";
  if (pct > 99.9) return ">99.9%";
  return `${pct.toFixed(1)}%`;
}

const plural = (n: number, one: string, many: string) => `${n === 1 ? one : many}`;

/**
 * The format and the one fact that distinguishes two tables of it: the log
 * version for Delta and Iceberg, one file or a directory for Parquet.
 */
function formatMark(table: EngineTable, version: SourceVersion | null): string {
  if (version?.kind === "delta_version") return `v${version.version}`;
  if (version?.kind === "iceberg_snapshot") return version.snapshot_id != null ? `#${version.snapshot_id}` : "";
  return formatKind(table.format, version);
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
  const [attentionOnly, setAttentionOnly] = useState(false);
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
      const hydrate = async (group: Group) => {
        const tables = await fetchTableDefinitions(group.schemaId).catch(() => [] as EngineTable[]);
        if (!live.current) return;
        setGroups(current => (current ?? []).map(g => g.schemaId === group.schemaId ? { ...g, tables } : g));
        if (!tables.length) {
          setMeasuring(current => ({ ...current, [group.schemaId]: "done" }));
          return;
        }
        setMeasuring(current => ({ ...current, [group.schemaId]: "loading" }));
        try {
          let list = await fetchInventory(group.schemaId, refresh);
          // A catalog inventory is useful only when its basic facts are
          // present. Metadata ANALYZE reads Parquet footers/manifests (no data
          // pages), so an Admin opening the page can establish those facts in
          // one pass. Deeper sketches and cubes remain explicit actions.
          if (list.some(entry => entry.state === "unmeasured")) {
            await Promise.allSettled(list
              .filter(entry => entry.state === "unmeasured")
              .map(entry => analyzeTable(entry.tableId, {})));
            list = await fetchInventory(group.schemaId, true);
          }
          if (!live.current) return;
          setMeasured(current => ({
            ...current,
            [group.schemaId]: Object.fromEntries(list.map(entry => [entry.tableId, entry])),
          }));
          setMeasuring(current => ({ ...current, [group.schemaId]: "done" }));
        } catch {
          if (live.current) setMeasuring(current => ({ ...current, [group.schemaId]: "failed" }));
        }
      };
      // The catalog structure is the navigation-critical payload. Statistics
      // can involve one metadata request per schema (and an automatic footer
      // read for unmeasured tables), so hydrate it after the table list is
      // visible. Explicit refreshes still wait for a complete inventory.
      const pending = Promise.all(flat.map(hydrate));
      if (refresh) await pending;
    } catch (e) {
      if (live.current) {
        setError(e instanceof CatalogError ? e : new CatalogError(0, "The catalog could not be read."));
      }
    }
  }, [catalogName, schemaName, isAdmin]);

  // Render the catalog structure immediately. Footer statistics hydrate in
  // the background; the explicit refresh button remains a synchronous
  // inventory operation.
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

  /**
   * The header's figures, over everything the search leaves. Rows are summed
   * only where they were measured, and split by standing so the coverage rule
   * below can show how much of the lake answers from what. A table nobody has
   * measured contributes no rows to any segment: its count is stated instead.
   */
  const totals = useMemo(() => {
    const t = {
      tables: matching.length, all: rows.length, measured: 0, attention: 0,
      // Unfiltered, every schema counts, including one holding nothing: the
      // figure then agrees with the rail beside it. A search narrows it to
      // the schemas the search actually reached.
      schemas: term ? new Set(matching.map(row => row.group.schemaId)).size : (groups ?? []).length,
      rows: 0, bytes: 0, files: 0,
      sketched: 0, sketchRows: 0, baseRows: 0, staleRows: 0,
    };
    for (const row of matching) {
      const standing = standingOf(row.measured);
      if (needsAttention(standing)) t.attention += 1;
      if (row.measured?.state !== "measured") continue;
      t.measured += 1;
      t.rows += row.measured.rows ?? 0;
      t.bytes += row.measured.bytes ?? 0;
      t.files += row.measured.files ?? 0;
      if (standing === "sketches") { t.sketched += 1; t.sketchRows += row.measured.rows ?? 0; }
      else if (standing === "stale") t.staleRows += row.measured.rows ?? 0;
      else t.baseRows += row.measured.rows ?? 0;
    }
    return t;
  }, [matching, rows.length, groups, term]);

  // The filter can only be on while there is something to filter to, so a
  // table that is measured while it is on returns the full list by itself.
  const onlyAttention = attentionOnly && totals.attention > 0;
  const visible = useMemo(() => !onlyAttention
    ? matching
    : matching.filter(row => needsAttention(standingOf(row.measured))), [matching, onlyAttention]);

  /** Sections keep their address order; only the rows inside them are ranked. */
  const narrowed = !!term || onlyAttention;
  const sections = useMemo(() => (groups ?? []).map(group => ({
    group,
    rows: visible.filter(row => row.group.schemaId === group.schemaId).sort(compare(sort)),
  })).filter(section => !narrowed || section.rows.length > 0), [groups, visible, sort, narrowed]);

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
      <header className={s.band}>
        <div className={s.bandTop}>
          <h1 className={s.bandTitle}>{schemaName ?? "Tables"}</h1>
          <div className={s.bandActions}>
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

        <div className={s.figures} aria-live="polite">
          <Figure
            value={term ? `${count(totals.tables)} of ${count(totals.all)}` : count(totals.tables)}
            label={plural(totals.tables, "table", "tables")}
            note={schemaName
              ? `in ${schemaName}`
              : `in ${count(totals.schemas)} ${plural(totals.schemas, "schema", "schemas")}`}
          />
          <Figure
            value={count(totals.rows)} label="rows"
            note={reading ? "still reading" : `${count(totals.measured)} of ${count(totals.tables)} measured`}
          />
          <Figure
            value={bytes(totals.bytes)} label="stored"
            note={`${count(totals.files)} ${plural(totals.files, "file", "files")}`}
          />
          <Figure
            value={count(totals.sketched)} label="with sketches"
            note={totals.sketched > 0 ? `${count(totals.sketchRows)} rows` : "none built yet"}
            strong={totals.sketched > 0}
          />
        </div>

        <Coverage
          sketchRows={totals.sketchRows} baseRows={totals.baseRows} staleRows={totals.staleRows}
          rows={totals.rows} tables={totals.tables} measured={totals.measured}
          attention={totals.attention} attentionOnly={onlyAttention}
          onAttention={() => setAttentionOnly(current => !current)}
        />
      </header>

      {!reading && sections.length === 0 && (
        <div className={s.empty}>
          <h2 className={s.emptyTitle}>
            {onlyAttention ? "Nothing needs attention" : term ? "Nothing matches that" : "No schemas yet"}
          </h2>
          <p className={s.emptyBody}>
            {onlyAttention
              ? "Every table in view has statistics that describe its source as it stands."
              : term
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
                <th className={s.cFiles}>Files</th>
                <SortHeader label="Changed" sortKey="changed" sort={sort} onSort={resort} className={s.cWhen} />
                <th className={s.cStand}>Acceleration</th>
              </tr>
            </thead>
            {sections.map(({ group, rows: sectionRows }) => (
              <tbody key={group.schemaId} className={s.schemaBody}>
                {/* Scoped to one schema, the heading above already names it;
                    a band repeating it would be the third place it appears. */}
                {!schemaName && (
                  <SchemaBand
                    group={group} rows={sectionRows} narrowed={narrowed} isEditor={isEditor}
                  />
                )}
                {group.tables === null && [0, 1, 2].map(index => <SkeletonRow key={index} />)}
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

/** One measured figure: the number, what it counts, and the fact that qualifies it. */
function Figure({ value, label, note, strong }: {
  value: string; label: string; note: string; strong?: boolean;
}) {
  return (
    <div className={s.figure}>
      <span className={`${s.figureValue} ${strong ? s.figureValueOn : ""}`}>{value}</span>
      <span className={s.figureLabel}>{label}</span>
      <span className={s.figureNote}>{note}</span>
    </div>
  );
}

/**
 * How much of the lake answers from what, measured in rows rather than tables,
 * because one table of five hundred million and fifteen of a few thousand are
 * not two halves of anything. The rule is the shape of the lake; the figures
 * beside it are the precise reading, since a segment of a tenth of a percent
 * is drawn at a legible minimum width rather than at its true one.
 */
function Coverage({ sketchRows, baseRows, staleRows, rows, tables, measured, attention, attentionOnly, onAttention }: {
  sketchRows: number; baseRows: number; staleRows: number; rows: number;
  tables: number; measured: number; attention: number;
  attentionOnly: boolean; onAttention: () => void;
}) {
  if (!tables) return null;
  const unmeasured = tables - measured;
  return (
    <div className={s.coverage}>
      <div
        className={s.rule} role="img"
        aria-label={`Of ${count(rows)} measured rows, ${count(sketchRows)} carry sketches and ${count(baseRows)} carry metadata statistics.`}
      >
        {sketchRows > 0 && <span className={`${s.seg} ${s.segSketch}`} style={{ flexGrow: sketchRows }} />}
        {baseRows > 0 && <span className={`${s.seg} ${s.segBase}`} style={{ flexGrow: baseRows }} />}
        {staleRows > 0 && <span className={`${s.seg} ${s.segStale}`} style={{ flexGrow: staleRows }} />}
        {rows === 0 && <span className={`${s.seg} ${s.segNone}`} style={{ flexGrow: 1 }} />}
      </div>
      <ul className={s.legend}>
        {sketchRows > 0 && (
          <li>
            <span className={`${s.key} ${s.segSketch}`} aria-hidden="true" />
            Sketches and cube cells
            <b>{count(sketchRows)}</b>
            <span className={s.legendShare}>{share(sketchRows, rows)}</span>
          </li>
        )}
        {baseRows > 0 && (
          <li>
            <span className={`${s.key} ${s.segBase}`} aria-hidden="true" />
            Counts, totals and bounds
            <b>{count(baseRows)}</b>
            <span className={s.legendShare}>{share(baseRows, rows)}</span>
          </li>
        )}
        {staleRows > 0 && (
          <li>
            <span className={`${s.key} ${s.segStale}`} aria-hidden="true" />
            Read in full, statistics stale
            <b>{count(staleRows)}</b>
            <span className={s.legendShare}>{share(staleRows, rows)}</span>
          </li>
        )}
        {unmeasured > 0 && (
          <li className={s.legendQuiet}>
            <span className={`${s.key} ${s.segNone}`} aria-hidden="true" />
            Not measured, so not counted
            <b>{count(unmeasured)} {plural(unmeasured, "table", "tables")}</b>
          </li>
        )}
        {attention > 0 && (
          <li className={s.legendAction}>
            <button type="button" className={`${s.chip} ${attentionOnly ? s.chipOn : ""}`}
              aria-pressed={attentionOnly} onClick={onAttention}>
              <i className="fas fa-triangle-exclamation" aria-hidden="true" />
              {count(attention)} {plural(attention, "table needs", "tables need")} attention
            </button>
          </li>
        )}
      </ul>
    </div>
  );
}

/**
 * A schema is a band across the one table, not a grid of its own. It carries
 * what it holds, so the band is a reading rather than a divider, and an empty
 * schema says so in the same place the counts would have gone — the row grid
 * below it stays a row grid.
 */
function SchemaBand({ group, rows, narrowed, isEditor }: {
  group: Group; rows: Row[]; narrowed: boolean; isEditor: boolean;
}) {
  const seen = rows.filter(row => row.measured?.state === "measured");
  const totalRows = seen.reduce((sum, row) => sum + (row.measured?.rows ?? 0), 0);
  const totalBytes = seen.reduce((sum, row) => sum + (row.measured?.bytes ?? 0), 0);
  const empty = group.tables !== null && group.tables.length === 0;
  return (
    <tr className={s.groupRow}>
      <th colSpan={8} scope="colgroup">
        <div className={s.groupBand}>
          <span className={s.groupName}>
            <span className={s.groupCatalog}>{group.catalog}.</span>{group.schema}
          </span>
          <span className={s.groupMeta}>
            {empty ? (
              <>
                <span className={s.groupQuiet}>No tables registered</span>
                {isEditor && (
                  <Link href={`/catalog/${enc(group.catalog)}/${enc(group.schema)}`} className={s.groupLink}>
                    Register one
                  </Link>
                )}
              </>
            ) : group.tables === null ? (
              <span className={s.groupQuiet}>Reading</span>
            ) : (
              <>
                <span>
                  {narrowed ? `${count(rows.length)} of ${count(group.tables.length)} ` : `${count(rows.length)} `}
                  {plural(narrowed ? group.tables.length : rows.length, "table", "tables")}
                </span>
                {seen.length > 0 && <span className={s.groupNum}>{count(totalRows)} rows</span>}
                {seen.length > 0 && <span className={s.groupNum}>{bytes(totalBytes)}</span>}
              </>
            )}
          </span>
        </div>
      </th>
    </tr>
  );
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
      <td className={s.cFormat}><span className={s.skel} style={{ width: 52 }} /></td>
      <td className={s.cRows}><span className={s.skel} style={{ width: 62 }} /></td>
      <td className={s.cSize}><span className={s.skel} style={{ width: 44 }} /></td>
      <td className={s.cFiles}><span className={s.skel} style={{ width: 20 }} /></td>
      <td className={s.cWhen}><span className={s.skel} style={{ width: 54 }} /></td>
      <td className={s.cStand} />
    </tr>
  );
}

function TableRow({ row, pending, expanded, onToggle, isEditor, isAnalyst, onMeasured }: {
  row: Row; pending: boolean; expanded: boolean; onToggle: () => void;
  isEditor: boolean; isAnalyst: boolean; onMeasured: (entry: Measurement) => void;
}) {
  const { group, table, measured } = row;
  const standing = standingOf(measured);
  const state = standing ? STANDING[standing] : null;
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
  const mark = formatMark(table, version);

  return (
    <>
      <tr className={`${s.invRow} ${expanded ? s.invRowOpen : ""}`}>
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
            {shape && (
              <span className={`${s.mark} ${s.markShape}`}
                title={`A cube shape is declared over this table: ${shape.dimensions?.length ?? 0} dimensions, ${shape.measures?.length ?? 0} measures`}>
                shaped
              </span>
            )}
          </span>
          <span className={s.narrowFacts} aria-hidden="true">
            <span>{[formatLabel(table.format), mark].filter(Boolean).join(" ")}</span>
            {measured?.state === "measured" && <span>{count(measured.rows)} rows</span>}
            {measured?.state === "measured" && <span>{bytes(measured.bytes)}</span>}
          </span>
        </td>
        <td className={s.cFormat}>
          {formatLabel(table.format)}
          {mark && <span className={s.formatMark}>{mark}</span>}
        </td>
        <td className={s.cRows}>{measured?.state === "measured" ? count(measured.rows) : waiting(62)}</td>
        <td className={s.cSize}>{measured?.state === "measured" ? bytes(measured.bytes) : waiting(44)}</td>
        <td className={s.cFiles}>{typeof files === "number" ? count(files) : pending ? <span className={s.skel} style={{ width: 20 }} /> : ""}</td>
        <td className={s.cWhen} title={exactTime(measured?.lastModifiedMs)}>
          {typeof measured?.lastModifiedMs === "number" ? since(measured.lastModifiedMs) : waiting(54)}
        </td>
        <td className={s.cStand}>
          {state?.word && (
            <span className={`${s.stand} ${state.tone}`} title={state.says}>{state.word}</span>
          )}
        </td>
      </tr>
      {expanded && (
        <tr className={s.detailRow}>
          <td colSpan={8} id={`d-${table.id}`}>
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
  row: Row; state: typeof STANDING[Standing] | null; version: SourceVersion | null; shape: EngineTable["shape"] | null;
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
