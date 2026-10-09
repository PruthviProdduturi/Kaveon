"use client";

/**
 * The inventory — every table Kaveon can answer over, and what it has
 * measured about each one.
 *
 * This page is the platform's measurement record, not a file browser. Every
 * figure on it is an observation with a provenance: a source version and the
 * moment it was taken. The one thing those observations establish is what can
 * be answered without reading data — statistics answer counts, totals and
 * bounds; sketches and cube cells answer distincts, quantiles and grouped
 * totals; a table with neither is read in full every time it is asked
 * anything.
 *
 * Three things govern how it is drawn.
 *
 * The proportion is in the numerals themselves. The page opens on one run —
 * answered over measured — rather than on two figures side by side with a
 * rule under them, because when 504,000,027 of 504,685,780 rows answer
 * without a scan the two numbers are nearly the same digits, and reading
 * them as a fraction is the finding. A big number with a small caption is
 * the default treatment of any statistic anywhere; a fraction is this one.
 *
 * Mass is structure. A row's share of the measure being ranked runs as a
 * rule under its own name, so the table that is most of the lake is visibly
 * most of the lake and a five-row table is visibly a tick. The scale is
 * linear, because a curve drawn to make the small tables look substantial
 * would hide the asymmetry that is the whole subject; a measured table keeps
 * a visible minimum instead, so "almost none" and "none" stay apart. Address
 * order would misreport mass, so schemas and the rows inside them are ranked
 * by the measure being sorted.
 *
 * The exception speaks and the rule is silent. Most tables answer counts and
 * bounds; that is what a table is unless something has been done to it, and
 * writing it on every row buries the two that answer more. Only a table that
 * is accelerated, stale, unmeasured or unreadable is given words, and the
 * accent belongs to the first of those and to nothing else on the page.
 *
 * Acceleration decays, which is the reason this page exists rather than a
 * tree. A table's statistics describe the version they were computed over,
 * and the source moves on. So the list carries two times, not one — when the
 * source last changed and when it was last measured — and their disagreement
 * is staleness, stated in the row and explained in its detail.
 *
 * Two reads compose one row. The Engine's table definitions arrive first and
 * are cheap — they draw every row and reserve the space the measured cells
 * will occupy. The per-schema inventory follows and fills those cells.
 * Nothing moves when the second read lands, and a table nobody has analyzed
 * shows an em dash, never a zero.
 */

import Link from "next/link";
import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useRole } from "../../hooks/useRole";
import { RegisterSheet, RemoveSchemaDialog } from "./CatalogEditor";
import s from "./catalog.module.css";
import {
  AnalyzeDepth, CatalogDefinition, CatalogError, EngineTable, Measurement, SchemaDefinition,
  SourceVersion, analyzeTable, bytes, count, deleteSchema, elapsed, enc, exactTime,
  fetchDefinitions, fetchInventory, fetchSchemaDefinitions, fetchTableDefinitions, formatLabel,
  labHref, since, versionDigest, versionLabel,
} from "./lib";
import { isSystemCatalog } from "../../utils/systemCatalog";

type SortKey = "name" | "rows" | "bytes" | "changed" | "measured";
type Sort = { key: SortKey; desc: boolean };
type Group = { catalogId: string; catalog: string; schemaId: string; schema: string; revision: number; tables: EngineTable[] | null };
interface Row { group: Group; table: EngineTable; measured: Measurement | undefined }

/**
 * What a question over this table costs, which is the only thing the catalog
 * is really for. `fast` is the product's claim; everything else is a reading
 * of the table, and the words say which. `baseline` marks the one reading
 * that is the norm rather than news: it is drawn as a rule, not written out,
 * because a word repeated down every row hides the two that are not it.
 */
const ANSWERING = {
  fast: {
    cell: "Sketches and cells", tone: s.ansFast, baseline: false,
    says: "Every column was read and its distinct-count and quantile sketches kept, so distinct counts, quantiles and the grouped totals of the declared shape are answered from precomputed values instead of from the rows.",
  },
  statistics: {
    cell: "Counts and bounds", tone: s.ansBase, baseline: true,
    says: "The statistics on record describe the source as it stands, so counts, totals and column bounds are answered without reading the data. Anything else reads the table.",
  },
  stale: {
    cell: "Statistics stale", tone: s.ansStale, baseline: false,
    says: "The source changed after these statistics were computed, so the figures in this row describe an earlier version of the table and the counts, totals and bounds on record no longer answer for it. Measuring it again puts the current reading on record.",
  },
  unmeasured: {
    cell: "Not measured", tone: s.ansNone, baseline: false,
    says: "This table has never been analyzed, so every question about it is answered by reading it.",
  },
  unreadable: {
    cell: "Unreadable", tone: s.ansBad, baseline: false,
    says: "The Engine could not read this location.",
  },
} as const;
type Answering = keyof typeof ANSWERING;

function answeringOf(measured: Measurement | undefined): Answering | null {
  if (!measured) return null;
  if (measured.state === "unreadable") return "unreadable";
  if (measured.state === "unmeasured") return "unmeasured";
  if (measured.stale) return "stale";
  return measured.depth === "full" ? "fast" : "statistics";
}

/** The three readings somebody has to act on, and the only ones worth filtering to. */
const needsAttention = (answering: Answering | null) =>
  answering === "stale" || answering === "unmeasured" || answering === "unreadable";

/** The depth a table already carries, so re-measuring it never quietly takes its sketches away. */
const depthOnRecord = (measured: Measurement | undefined): AnalyzeDepth =>
  measured?.depth === "full" ? { sketches: true } : {};

/** The three forms of ANALYZE, each in the one sentence that says what it costs and what it buys. */
const ANALYZE_FORMS: { label: string; depth: AnalyzeDepth; needsShape?: boolean; blurb: string }[] = [
  {
    label: "Metadata", depth: {},
    blurb: "Reads the Delta log, the Iceberg manifests or the Parquet footers and no data pages, so it finishes in seconds and puts row counts, size and column bounds on record.",
  },
  {
    label: "Sketches", depth: { sketches: true },
    blurb: "Reads every column once to build distinct-count and quantile sketches, so how many distinct values a column holds is answered later without reading the table again.",
  },
  {
    label: "Cube", depth: { sketches: true, cube: true }, needsShape: true,
    blurb: "Builds the cells of the table's declared shape in the same scan, so grouped totals over those dimensions are answered from the cells instead of the rows.",
  },
];

/** A share of a whole, to one decimal, without rounding a part into the whole or out of existence. */
function share(part: number, whole: number): string {
  if (!whole) return "—";
  if (part === whole) return "100%";
  if (part === 0) return "0%";
  const pct = (part / whole) * 100;
  if (pct > 0 && pct < 0.1) return "<0.1%";
  if (pct > 99.9) return ">99.9%";
  return `${pct.toFixed(1)}%`;
}

const plural = (n: number, one: string, many: string) => (n === 1 ? one : many);

/** The one fact that separates two tables of the same format: the log version. */
function formatMark(version: SourceVersion | null): string {
  if (version?.kind === "delta_version") return `v${version.version}`;
  if (version?.kind === "iceberg_snapshot") return version.snapshot_id != null ? `#${version.snapshot_id}` : "";
  // Nothing for a plain directory or a single file: the Holds/files count
  // beside it already says which it is, and the words were long enough that
  // the cell clipped them to "di…" and "si…". A version or a snapshot id is
  // the only part of this a reader cannot get from another column.
  if (version?.kind === "listing" || version?.kind === "file") return "";
  return "";
}

/** The declared shape, in the terms the cube is actually built over. */
function shapeLine(shape: EngineTable["shape"] | null | undefined): string | null {
  if (!shape?.dimensions?.length) return null;
  const time = shape.time as { column?: string; grain?: string } | null | undefined;
  const parts = [
    `${shape.dimensions.length} ${plural(shape.dimensions.length, "dimension", "dimensions")}`,
    `${shape.measures?.length ?? 0} ${plural(shape.measures?.length ?? 0, "measure", "measures")}`,
  ];
  if (time?.column && time.grain) parts.push(`${time.grain} grain on ${time.column}`);
  return parts.join(", ");
}

/**
 * The measure a row's mass bar is drawn in. Whatever the list is ranked by is
 * what the bar shows, so the bar and the ordering can never tell a reader two
 * different stories; ranked by name or by a time, the bar falls back to what
 * the lake physically is.
 */
const massKey = (sort: Sort): "rows" | "bytes" => (sort.key === "rows" ? "rows" : "bytes");

export function Inventory({ catalogName, schemaName, action }: {
  catalogName?: string; schemaName?: string;
  /** The one thing this list can be missing, offered beside the list itself. */
  action?: React.ReactNode;
}) {
  const { isAdmin, isEditor } = useRole();
  const [catalogs, setCatalogs] = useState<CatalogDefinition[] | null>(null);
  const [error, setError] = useState<CatalogError | null>(null);
  const [groups, setGroups] = useState<Group[] | null>(null);
  const [measured, setMeasured] = useState<Record<string, Record<string, Measurement>>>({});
  const [measuring, setMeasuring] = useState<Record<string, "loading" | "done" | "failed">>({});
  const [query, setQuery] = useState("");
  // Mass first. A catalog where one table is most of the lake is misreported
  // by an alphabet, and the reader who opens this page is looking for the
  // thing that costs something.
  const [sort, setSort] = useState<Sort>({ key: "rows", desc: true });
  const [open, setOpen] = useState<string | null>(null);
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(null);
  const [attentionOnly, setAttentionOnly] = useState(false);
  const [registerIn, setRegisterIn] = useState<Group | null>(null);
  const [removeSchema, setRemoveSchema] = useState<{ group: Group; busy: boolean; error: string | null } | null>(null);
  const live = useRef(true);

  useEffect(() => {
    live.current = true;
    return () => { live.current = false; };
  }, []);

  /** Definitions first, then the measurements that fill the cells they drew. */
  const load = useCallback(async (refresh: boolean) => {
    try {
      // This list is the lake: the catalogs a reader registers tables into
      // and measures. KaveonDB is neither — it holds the platform's own
      // records, nobody registers into it, and its one empty Engine schema
      // would otherwise appear here under a header offering to add tables to
      // it. It is read beside this list instead, by SystemCatalog.
      const definitions = (await fetchDefinitions()).filter(
        definition => !isSystemCatalog(definition.name));
      if (!live.current) return;
      setCatalogs(definitions);
      setError(null);
      const listed = await Promise.all(definitions.map(async catalog => {
        const schemas = await fetchSchemaDefinitions(catalog.id).catch(() => [] as SchemaDefinition[]);
        return schemas
          .filter(schema => !schemaName || (catalog.name === catalogName && schema.name === schemaName))
          .map<Group>(schema => ({
            catalogId: catalog.id, catalog: catalog.name,
            schemaId: schema.id, schema: schema.name, revision: schema.revision, tables: null,
          }));
      }));
      const flat = listed.flat();
      if (!live.current) return;
      setGroups(flat);
      if (refresh) setProgress({ done: 0, total: flat.length });
      const hydrate = async (group: Group) => {
        const tables = await fetchTableDefinitions(group.schemaId).catch(() => [] as EngineTable[]);
        if (!live.current) return;
        setGroups(current => (current ?? []).map(g => g.schemaId === group.schemaId ? { ...g, tables } : g));
        if (!tables.length) {
          setMeasuring(current => ({ ...current, [group.schemaId]: "done" }));
          if (refresh) setProgress(current => current && { ...current, done: current.done + 1 });
          return;
        }
        setMeasuring(current => ({ ...current, [group.schemaId]: "loading" }));
        try {
          // The inventory endpoint establishes the bounded footer fact for any
          // table that has never been analyzed before it answers, so a second
          // pass from here would repeat work the server has already done.
          const list = await fetchInventory(group.schemaId, refresh);
          if (!live.current) return;
          setMeasured(current => ({
            ...current,
            [group.schemaId]: Object.fromEntries(list.map(entry => [entry.tableId, entry])),
          }));
          setMeasuring(current => ({ ...current, [group.schemaId]: "done" }));
        } catch {
          if (live.current) setMeasuring(current => ({ ...current, [group.schemaId]: "failed" }));
        } finally {
          if (live.current && refresh) setProgress(current => current && { ...current, done: current.done + 1 });
        }
      };
      // The catalog structure is the navigation-critical payload; statistics
      // are one metadata request per schema, so they hydrate after the list is
      // visible. An explicit re-measure waits for a complete inventory and
      // reports how much of it has landed.
      const pending = Promise.all(flat.map(hydrate));
      if (refresh) await pending;
    } catch (e) {
      if (live.current) {
        setError(e instanceof CatalogError ? e : new CatalogError(0, "The catalog could not be read."));
      }
    }
  }, [catalogName, schemaName]);

  // Render the catalog structure immediately; the measurements follow.
  useEffect(() => { void load(false); }, [load]);

  const refresh = async () => {
    await load(true);
    if (live.current) setProgress(null);
  };

  const confirmRemoveSchema = async () => {
    if (!removeSchema) return;
    const { group } = removeSchema;
    setRemoveSchema({ group, busy: true, error: null });
    try {
      await deleteSchema(group.schemaId, group.revision);
      if (!live.current) return;
      setRemoveSchema(null);
      await load(false);
    } catch (e) {
      if (live.current) {
        setRemoveSchema({
          group, busy: false,
          error: e instanceof CatalogError ? e.message : "The schema could not be removed.",
        });
      }
    }
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
   * The page's position, over everything the search leaves: how many measured
   * rows answer from precomputed values and how many are read. A table nobody
   * has measured contributes no rows to either — its count is stated instead.
   */
  const totals = useMemo(() => {
    const t = {
      tables: matching.length, all: rows.length, measured: 0,
      // Unfiltered, every schema counts, including one holding nothing: the
      // figure then agrees with the rail beside it. A search narrows it to
      // the schemas the search actually reached.
      schemas: term ? new Set(matching.map(row => row.group.schemaId)).size : (groups ?? []).length,
      rows: 0, bytes: 0, files: 0,
      fast: 0, fastRows: 0, baseTables: 0, baseRows: 0, staleTables: 0, staleRows: 0,
      attention: [] as Row[],
    };
    for (const row of matching) {
      const answering = answeringOf(row.measured);
      if (needsAttention(answering)) t.attention.push(row);
      if (row.measured?.state !== "measured") continue;
      t.measured += 1;
      t.rows += row.measured.rows ?? 0;
      t.bytes += row.measured.bytes ?? 0;
      t.files += row.measured.files ?? 0;
      if (answering === "fast") { t.fast += 1; t.fastRows += row.measured.rows ?? 0; }
      else if (answering === "stale") { t.staleTables += 1; t.staleRows += row.measured.rows ?? 0; }
      else { t.baseTables += 1; t.baseRows += row.measured.rows ?? 0; }
    }
    t.attention.sort((a, b) => (b.measured?.rows ?? 0) - (a.measured?.rows ?? 0));
    return t;
  }, [matching, rows.length, groups, term]);

  // The filter can only be on while there is something to filter to, so a
  // table that is measured while it is on returns the full list by itself.
  const onlyAttention = attentionOnly && totals.attention.length > 0;
  const visible = useMemo(() => !onlyAttention
    ? matching
    : matching.filter(row => needsAttention(answeringOf(row.measured))), [matching, onlyAttention]);

  /**
   * Sections are ranked by the same measure the rows are, so the schema
   * holding most of what is being sorted is read first. Sorting by name is
   * the one case where address order is the ranking.
   */
  const narrowed = !!term || onlyAttention;
  const sections = useMemo(() => {
    const built = (groups ?? []).map(group => {
      const sectionRows = visible.filter(row => row.group.schemaId === group.schemaId).sort(compare(sort));
      const seen = sectionRows.filter(row => row.measured?.state === "measured");
      const sum = (pick: (row: Row) => number | null | undefined) =>
        seen.reduce((total, row) => total + (pick(row) ?? 0), 0);
      return {
        group, rows: sectionRows, seen: seen.length,
        rowTotal: sum(row => row.measured?.rows),
        byteTotal: sum(row => row.measured?.bytes),
        fileTotal: sum(row => row.measured?.files),
      };
    });
    if (sort.key !== "name") {
      const rank = (section: typeof built[number]) =>
        sort.key === "bytes" ? section.byteTotal : section.rowTotal;
      built.sort((a, b) => (rank(b) - rank(a)) || a.group.schema.localeCompare(b.group.schema));
    }
    return built.filter(section => !narrowed || section.rows.length > 0);
  }, [groups, visible, sort, narrowed]);

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
        {/* KaveonDB is always present and is not one of these, so the
            heading says which kind of catalog is missing. */}
        <h1 className={s.emptyTitle}>{isAdmin ? "No data catalogs registered" : "No data catalogs available to you"}</h1>
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
  const measuringNow = progress !== null;
  // The denominator every row's mass bar is drawn against: the whole of what
  // the search left, in the measure the list is ranked by.
  const mass = massKey(sort);
  const massTotal = mass === "rows" ? totals.rows : totals.bytes;

  return (
    <>
      <header className={s.band}>
        <div className={s.bandTop}>
          {/* The rail beside this page already says Catalog, and a title set
              smaller than the figures under it only flattens the page. A
              schema is different: nothing else on screen names it, so there
              it is written. The heading stays in the document either way,
              because a page with no heading is a page a screen reader cannot
              describe. */}
          <h1 className={schemaName ? s.bandTitle : s.sr}>{schemaName ?? "Catalog"}</h1>
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
            <button type="button" className={s.ghost} onClick={refresh} disabled={measuringNow}>
              <i className={`fas fa-rotate ${measuringNow ? s.rotate : ""}`} aria-hidden="true" />
              {measuringNow
                ? `Measuring ${count(progress.done)} of ${count(progress.total)}`
                : "Re-measure"}
            </button>
            {action}
          </div>
        </div>

        <Lake
          fastRows={totals.fastRows} baseRows={totals.baseRows} staleRows={totals.staleRows}
          fastTables={totals.fast} baseTables={totals.baseTables} staleTables={totals.staleTables}
          tables={totals.tables} all={totals.all} narrowed={!!term}
          schemas={totals.schemas} bytes={totals.bytes} files={totals.files}
          outstanding={totals.tables - totals.measured} reading={reading} schemaName={schemaName}
        />

        <Exception
          attention={totals.attention} only={onlyAttention}
          onToggle={() => setAttentionOnly(current => !current)}
        />
      </header>

      {!reading && sections.length === 0 && (
        <div className={s.empty}>
          <h2 className={s.emptyTitle}>
            {onlyAttention ? "Nothing needs measuring" : term ? "Nothing matches that" : "No schemas yet"}
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
            {/* Fixed layout reads its widths from the column elements, so the
                two-row head can group columns without the group's width being
                shared out equally among the columns under it. */}
            <colgroup>
              <col className={s.colName} />
              <col className={s.colFormat} />
              <col className={s.colRows} />
              <col className={s.colSize} />
              <col className={s.colFiles} />
              <col className={s.colChanged} />
              <col className={s.colMeasured} />
              <col className={s.colAnswer} />
            </colgroup>
            {/* Eight columns are four readings: what the table is, what it
                holds, when it was last touched, and what it can answer. The
                head says so, which is what keeps the list from reading as a
                comb of eight equal slots. */}
            <thead>
              <tr className={s.headGroup}>
                <SortHeader label="Table" sortKey="name" sort={sort} onSort={resort} className={s.cName} rowSpan={2} />
                <th className={s.cFormat} rowSpan={2}>Format</th>
                <th className={s.gHead} colSpan={3} scope="colgroup">Holds</th>
                <th className={s.gHead} colSpan={2} scope="colgroup">Last</th>
                <th className={s.cAnswer} rowSpan={2}>Answers</th>
              </tr>
              <tr className={s.headSub}>
                <SortHeader label="rows" sortKey="rows" sort={sort} onSort={resort} className={`${s.cRows} ${s.gStart}`} />
                <SortHeader label="size" sortKey="bytes" sort={sort} onSort={resort} className={s.cSize} />
                <th className={s.cFiles}>files</th>
                <SortHeader label="changed" sortKey="changed" sort={sort} onSort={resort} className={`${s.cWhen} ${s.cChanged} ${s.gStart}`} />
                <SortHeader label="measured" sortKey="measured" sort={sort} onSort={resort} className={s.cWhen} />
              </tr>
            </thead>
            {sections.map(({ group, rows: sectionRows, seen, rowTotal, byteTotal, fileTotal }) => (
              <tbody key={group.schemaId} className={s.schemaBody}>
                {/* Scoped to one schema, the heading above already names it;
                    a band repeating it would be the third place it appears. */}
                {!schemaName && (
                  <SchemaBand
                    group={group} rows={sectionRows} narrowed={narrowed} isEditor={isEditor} seen={seen}
                    rowTotal={rowTotal} byteTotal={byteTotal} fileTotal={fileTotal}
                    onRegister={() => setRegisterIn(group)}
                    onRemove={() => setRemoveSchema({ group, busy: false, error: null })}
                  />
                )}
                {group.tables === null && [0, 1, 2].map(index => <SkeletonRow key={index} />)}
                {sectionRows.map(row => (
                  <TableRow
                    key={row.table.id} row={row}
                    pending={measuring[row.group.schemaId] === "loading" && !row.measured}
                    expanded={open === row.table.id}
                    onToggle={() => setOpen(current => current === row.table.id ? null : row.table.id)}
                    isEditor={isEditor} mass={mass} massTotal={massTotal}
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

      {registerIn && (
        <RegisterSheet
          kind="table" catalog={registerIn.catalog} schema={registerIn.schema}
          onClose={() => { setRegisterIn(null); void load(false); }}
        />
      )}
      {removeSchema && (
        <RemoveSchemaDialog
          fullName={`${removeSchema.group.catalog}.${removeSchema.group.schema}`}
          busy={removeSchema.busy} error={removeSchema.error}
          onCancel={() => setRemoveSchema(null)} onConfirm={confirmRemoveSchema}
        />
      )}
    </>
  );
}

/**
 * What the lake can answer, as one fraction.
 *
 * Answered over measured, in one run of numerals: when 504,000,027 of
 * 504,685,780 rows answer without a scan the two figures are nearly the same
 * digits, and reading them against each other is the finding. Two figures
 * set side by side under separate captions are two facts; a fraction is one,
 * and it is the one the product is about.
 *
 * Under it, two quiet lines in the order a reader needs them: how many
 * tables stand behind the fraction and what the rest cost, then what the
 * lake physically is. The second is evidence, not finding, which is why it
 * follows rather than standing in the opposite corner competing.
 */
function Lake({ fastRows, baseRows, staleRows, fastTables, baseTables, staleTables, tables, all, narrowed, schemas, bytes: stored, files, outstanding, reading, schemaName }: {
  fastRows: number; baseRows: number; staleRows: number;
  fastTables: number; baseTables: number; staleTables: number;
  tables: number; all: number; narrowed: boolean;
  schemas: number; bytes: number; files: number;
  outstanding: number; reading: boolean; schemaName?: string;
}) {
  const total = fastRows + baseRows + staleRows;
  const scanned = baseRows + staleRows;
  const scannedTables = baseTables + staleTables;
  // Nothing has been measured yet and nothing is claimed: a fraction of
  // zero over zero would read as a finding where there is only a wait.
  const blank = total === 0;
  return (
    <div className={s.lake}>
      <p className={s.headline}>
        <span className={s.headFraction}>
          <span className={s.headAnswered}>{blank ? "—" : count(fastRows)}</span>
          <span className={s.headOver} aria-hidden="true">/</span>
          <span className={s.sr}>of</span>
          <span className={s.headMeasured}>{blank ? "—" : count(total)}</span>
        </span>
        <span className={s.headSays}>rows answer without a scan</span>
      </p>
      <p className={s.lakeStanding}>
        {blank ? (
          reading ? "Reading what has been measured." : "Nothing in view has been measured yet."
        ) : (
          <>
            <b>{count(fastTables)}</b> of <b>{count(tables)}</b> {plural(tables, "table", "tables")} answer
            this way, <b>{share(fastRows, total)}</b> of what is measured.
            {/* A figure and the noun it counts stay on one line: this read
                "hold 685,753" and then wrapped before "rows that are read in
                full", leaving a number dangling at the end of a line with
                nothing saying what it counted. */}
            {scannedTables > 0 && (
              <> The other <b>{count(scannedTables)}</b> hold <span className={s.nowrapPair}><b>{count(scanned)}</b> rows</span> that are read in full
                {staleTables > 0 && <>, <b>{count(staleTables)}</b> of them carrying statistics the source has moved past</>}.
              </>
            )}
          </>
        )}
        {reading && !blank && " Still reading."}
        {!reading && outstanding > 0 && ` ${count(outstanding)} ${plural(outstanding, "table has", "tables have")} no measurement on record.`}
      </p>
      <p className={s.lakeFacts}>
        <b>{narrowed ? `${count(tables)} of ${count(all)}` : count(tables)}</b>
        {` ${plural(narrowed ? all : tables, "table", "tables")}`}
        {!schemaName && <> across <b>{count(schemas)}</b> {plural(schemas, "schema", "schemas")}</>}
        {", "}
        <b>{bytes(stored)}</b> in <b>{count(files)}</b> {plural(files, "file", "files")}.
      </p>
    </div>
  );
}

/**
 * What is not current, named. One sentence about the table that costs the
 * most, a count of the rest, and the one control that narrows the list to
 * them. It is absent entirely while there is nothing to say.
 */
function Exception({ attention, only, onToggle }: {
  attention: Row[]; only: boolean; onToggle: () => void;
}) {
  if (!attention.length) return null;
  const [first, ...rest] = attention;
  const answering = answeringOf(first.measured);
  const name = `${first.group.schema}.${first.table.name}`;
  const rowsAtStake = first.measured?.rows;
  const lead = answering === "stale"
    ? `${name} changed after it was last measured${typeof rowsAtStake === "number" ? `, so its ${count(rowsAtStake)} rows are read until it is measured again` : ""}.`
    : answering === "unreadable"
      ? `${name} could not be read at its location.`
      : `${name} has never been analyzed, so every question about it reads the table.`;
  return (
    <div className={s.exception}>
      <i className="fas fa-triangle-exclamation" aria-hidden="true" />
      <p className={s.exceptionText}>
        {lead}
        {rest.length > 0 && ` ${count(rest.length)} other ${plural(rest.length, "table needs", "tables need")} measuring.`}
      </p>
      <button type="button" className={`${s.chip} ${only ? s.chipOn : ""}`} aria-pressed={only} onClick={onToggle}>
        {only ? "Show every table" : `Show only these ${count(attention.length)}`}
      </button>
    </div>
  );
}

/**
 * A schema is a band across the one table, not a grid of its own, and its
 * subtotals stand in the columns they total — the rows under rows, the bytes
 * under size, the files under files. Reading down a column therefore gives a
 * schema's share and a table's share in the same glance, which is the whole
 * reason the sections exist. An empty schema offers the two things that can
 * be done with it in the place the subtotals would have gone.
 */
function SchemaBand({ group, rows, narrowed, isEditor, seen, rowTotal, byteTotal, fileTotal, onRegister, onRemove }: {
  group: Group; rows: Row[]; narrowed: boolean; isEditor: boolean; seen: number;
  rowTotal: number; byteTotal: number; fileTotal: number;
  onRegister: () => void; onRemove: () => void;
}) {
  const empty = group.tables !== null && group.tables.length === 0;
  const held = group.tables === null
    ? "Reading"
    : empty
      ? "No tables registered"
      : `${narrowed ? `${count(rows.length)} of ${count(group.tables.length)}` : count(rows.length)} ${plural(narrowed ? group.tables.length : rows.length, "table", "tables")}`;
  return (
    <tr className={s.groupRow}>
      <th colSpan={2} scope="colgroup">
        <span className={s.groupBand}>
          <span className={s.groupName}>
            <span className={s.groupCatalog}>{group.catalog}</span>
            {group.schema}
          </span>
          <span className={s.groupHeld}>{held}</span>
          {/* Narrowed, the columns these subtotals stand in are gone, so they
              follow the schema's name the same way a row's figures follow
              its own. Wide, the columns carry them and this is not drawn. */}
          {seen > 0 && (
            <span className={s.groupNarrow} aria-hidden="true">
              <span>{count(rowTotal)} rows</span>
              <span>{bytes(byteTotal)}</span>
              <span>{count(fileTotal)} {plural(fileTotal, "file", "files")}</span>
            </span>
          )}
        </span>
      </th>
      <td className={`${s.cRows} ${s.gStart} ${s.groupNum}`}>{seen ? count(rowTotal) : ""}</td>
      <td className={`${s.cSize} ${s.groupNum}`}>{seen ? bytes(byteTotal) : ""}</td>
      <td className={`${s.cFiles} ${s.groupNum}`}>{seen ? count(fileTotal) : ""}</td>
      <td colSpan={3} className={s.groupTail}>
        {empty && isEditor && (
          <span className={s.groupActions}>
            <button type="button" className={s.groupAction} onClick={onRegister}>Register a table</button>
            <button type="button" className={s.groupAction} onClick={onRemove}>Remove schema</button>
          </span>
        )}
      </td>
    </tr>
  );
}

function SortHeader({ label, sortKey, sort, onSort, className, rowSpan }: {
  label: string; sortKey: SortKey; className: string; sort: Sort; onSort: (key: SortKey) => void;
  rowSpan?: number;
}) {
  const active = sort.key === sortKey;
  return (
    <th className={className} rowSpan={rowSpan} aria-sort={active ? (sort.desc ? "descending" : "ascending") : "none"}>
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
      : sort.key === "bytes" ? row.measured?.bytes
      : sort.key === "measured" ? row.measured?.computedAtMs
      : row.measured?.lastModifiedMs;
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
      <td className={s.cName}><span className={s.skel} style={{ width: 168 }} /></td>
      <td className={s.cFormat}><span className={s.skel} style={{ width: 62 }} /></td>
      <td className={`${s.cRows} ${s.gStart}`}><span className={s.skel} style={{ width: 84 }} /></td>
      <td className={s.cSize}><span className={s.skel} style={{ width: 56 }} /></td>
      <td className={s.cFiles}><span className={s.skel} style={{ width: 26 }} /></td>
      <td className={`${s.cWhen} ${s.cChanged} ${s.gStart}`}><span className={s.skel} style={{ width: 48 }} /></td>
      <td className={s.cWhen}><span className={s.skel} style={{ width: 48 }} /></td>
      <td className={s.cAnswer} />
    </tr>
  );
}

/**
 * A table's mass, in the measure the list is ranked by, drawn as a rule
 * under its own name — where the eye already is, and attached to the thing
 * it describes rather than standing off in a column of its own.
 *
 * Linear, against the whole of what is listed. A curve would make a
 * five-row table look like a share of a five-hundred-million-row one, and
 * that asymmetry is the subject of the page, not something to soften. A
 * measured table keeps a visible minimum instead, so "almost none of the
 * lake" and "none of it" stay apart, and a table with no measurement draws
 * nothing at all.
 */
function Mass({ part, whole, measure }: { part: number | null | undefined; whole: number; measure: "rows" | "bytes" }) {
  if (typeof part !== "number" || whole <= 0) return null;
  const pct = Math.max((part / whole) * 100, 0);
  const reading = measure === "rows" ? `${count(part)} of ${count(whole)} rows` : `${bytes(part)} of ${bytes(whole)}`;
  return (
    <span className={s.massTrack} aria-hidden="true" title={`${share(part, whole)} of what is listed — ${reading}.`}>
      <span className={s.massFill} style={{ width: `${pct}%` }} />
    </span>
  );
}

function TableRow({ row, pending, expanded, onToggle, isEditor, mass, massTotal, onMeasured }: {
  row: Row; pending: boolean; expanded: boolean; onToggle: () => void;
  isEditor: boolean; mass: "rows" | "bytes"; massTotal: number;
  onMeasured: (entry: Measurement) => void;
}) {
  const { group, table, measured } = row;
  const answering = answeringOf(measured);
  const state = answering ? ANSWERING[answering] : null;
  const current = measured?.currentSourceVersion ?? measured?.sourceVersion ?? null;
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
    : current?.kind === "listing" ? current.files
    : current?.kind === "file" ? 1 : null;
  const mark = formatMark(current);
  // A stale row's figures describe the version they were computed over, not
  // the source as it stands. They are kept — they are the only measurement
  // there is — and quietened, so nobody reads them as current.
  const past = answering === "stale";
  const asAt = past ? "Measured over an earlier version of this table." : undefined;

  return (
    <>
      <tr className={`${s.invRow} ${expanded ? s.invRowOpen : ""}`}>
        <td className={s.cName}>
          <span className={s.nameLine}>
            <button
              type="button" className={s.disclose} onClick={onToggle}
              aria-expanded={expanded} aria-controls={`d-${table.id}`}
              aria-label={`Details for ${table.name}`}
            >
              <span className={`${s.chev} ${expanded ? s.chevOpen : ""}`} aria-hidden="true">
                <i className="fas fa-chevron-right" />
              </span>
            </button>
            <Link href={href} className={s.tableName}>{table.name}</Link>
            {partitions.length > 0 && <span className={s.mark} title={`Partitioned by ${partitions.join(", ")}`}>partitioned</span>}
            {clustered.length > 0 && <span className={s.mark} title={`Clustered by ${clustered.join(", ")}`}>clustered</span>}
            {shape && (
              <span className={s.mark} title={`A cube shape is declared over this table: ${shapeLine(shape)}`}>
                shaped
              </span>
            )}
          </span>
          {measured?.state === "measured" && (
            <Mass part={mass === "rows" ? measured.rows : measured.bytes} whole={massTotal} measure={mass} />
          )}
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
        <td className={`${s.cRows} ${s.gStart} ${past ? s.cPast : ""}`} title={asAt}>
          {measured?.state === "measured" ? count(measured.rows) : waiting(84)}
        </td>
        <td className={`${s.cSize} ${past ? s.cPast : ""}`} title={asAt}>
          {measured?.state === "measured" ? bytes(measured.bytes) : waiting(56)}
        </td>
        <td className={`${s.cFiles} ${past ? s.cPast : ""}`} title={asAt}>
          {typeof files === "number" ? count(files) : pending ? <span className={s.skel} style={{ width: 26 }} /> : ""}
        </td>
        <td className={`${s.cWhen} ${s.cChanged} ${s.gStart}`} title={exactTime(measured?.lastModifiedMs)}>
          {typeof measured?.lastModifiedMs === "number" ? elapsed(measured.lastModifiedMs) : waiting(48)}
        </td>
        <td className={s.cWhen} title={exactTime(measured?.computedAtMs)}>
          {typeof measured?.computedAtMs === "number" ? elapsed(measured.computedAtMs) : waiting(48)}
        </td>
        {/* Counts and bounds is what a table answers unless something has
            been done to it, so this column says nothing for it and keeps its
            words for the exceptions. The reading is not lost: the cell
            carries it for a pointer, the row's detail writes it out in full,
            and a screen reader is given it here. */}
        <td className={s.cAnswer} title={state?.says}>
          {state && (state.baseline
            ? <span className={s.sr}>{state.cell}</span>
            : <span className={`${s.answers} ${state.tone}`}>{state.cell}</span>)}
        </td>
      </tr>
      {expanded && (
        <tr className={s.detailRow}>
          <td colSpan={8} id={`d-${table.id}`}>
            <Detail
              row={row} state={state} answering={answering} current={current} shape={shape}
              partitions={partitions} clustered={clustered}
              isEditor={isEditor} href={href} onMeasured={onMeasured}
            />
          </td>
        </tr>
      )}
    </>
  );
}

function Detail({ row, state, answering, current, shape, partitions, clustered, isEditor, href, onMeasured }: {
  row: Row; state: typeof ANSWERING[Answering] | null; answering: Answering | null;
  current: SourceVersion | null; shape: EngineTable["shape"] | null;
  partitions: string[]; clustered: string[];
  isEditor: boolean; href: string; onMeasured: (entry: Measurement) => void;
}) {
  const { group, table, measured } = row;
  const [running, setRunning] = useState<string | null>(null);
  const [startedAt, setStartedAt] = useState<number | null>(null);
  const [elapsed, setElapsed] = useState(0);
  const [outcome, setOutcome] = useState<{ ok: boolean; text: string } | null>(null);

  // A full read of a large table runs for minutes. The button that started it
  // says how long it has been running, so waiting is a measured thing rather
  // than a frozen control.
  useEffect(() => {
    if (startedAt === null) return;
    setElapsed(0);
    const id = window.setInterval(() => setElapsed(Math.round((Date.now() - startedAt) / 1000)), 1000);
    return () => window.clearInterval(id);
  }, [startedAt]);

  const run = async (label: string, depth: AnalyzeDepth) => {
    setRunning(label);
    setStartedAt(Date.now());
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
      setStartedAt(null);
    }
  };

  const again = answering === "stale" || answering === "unmeasured";
  const againDepth = depthOnRecord(measured);
  const againLabel = againDepth.sketches ? "Measure again, with sketches" : "Measure again";

  return (
    <div className={s.detail}>
      <dl className={s.facts}>
        <dt>Location</dt>
        <dd className={s.mono}>{table.location}</dd>

        <dt>Source now</dt>
        <dd>
          <span className={s.mono}>{versionLabel(current)}</span>
          {versionDigest(current) && <span className={s.digest}>{versionDigest(current)}</span>}
          {typeof measured?.observedAtMs === "number" && (
            <span className={s.aside} title={exactTime(measured.observedAtMs)}>Read {since(measured.observedAtMs)}.</span>
          )}
        </dd>

        <dt>Measured over</dt>
        <dd>
          {measured?.state === "measured" ? (
            <>
              <span className={s.mono}>{versionLabel(measured.sourceVersion)}</span>
              {versionDigest(measured.sourceVersion) && <span className={s.digest}>{versionDigest(measured.sourceVersion)}</span>}
              <span className={s.aside} title={exactTime(measured.computedAtMs)}>
                {`Measured ${since(measured.computedAtMs)}`}
                {measured.depth === "full"
                  ? ", every column read and the distinct-count and quantile sketches kept"
                  : ", from the source's own metadata with no data pages read"}
                {typeof measured.rowGroups === "number" ? `, ${count(measured.rowGroups)} row groups` : ""}
                {typeof measured.uncompressedBytes === "number" ? `, ${bytes(measured.uncompressedBytes)} uncompressed` : ""}
                {typeof measured.measuredColumns === "number" ? `, ${count(measured.measuredColumns)} columns described` : ""}.
              </span>
            </>
          ) : (
            <span className={s.dash}>—</span>
          )}
        </dd>

        <dt>Answers from</dt>
        <dd className={answering === "unreadable" ? s.factBad : undefined}>
          <span className={s.factLead}>{state?.cell}</span>
          {state?.says}
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
              {shapeLine(shape)}
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
      </div>

      {isEditor && (
        <div className={s.analyze}>
          <h3 className={s.analyzeTitle}>Measure this table</h3>
          {again && (
            <p className={s.analyzeLead}>
              {/* Re-measuring reuses the depth already on record, so a table
                  carrying sketches is never quietly reduced to a footer read. */}
              <button
                type="button" className={`${s.formBtn} ${s.formBtnLead}`} disabled={running !== null}
                onClick={() => run(againLabel, againDepth)}
              >
                {running === againLabel ? `Measuring, ${count(elapsed)}s` : againLabel}
              </button>
              <span>
                {againDepth.sketches
                  ? "Reads every column again and rebuilds the sketches, so this table keeps what it answers from today."
                  : "Reads the source's own metadata and puts the current row count, size and column bounds on record."}
              </span>
            </p>
          )}
          <ul className={s.forms}>
            {ANALYZE_FORMS.map(form => {
              const blocked = form.needsShape && !shape;
              return (
                <li key={form.label}>
                  <button
                    type="button" className={s.formBtn} disabled={running !== null || blocked}
                    onClick={() => run(form.label, form.depth)}
                  >
                    {running === form.label ? `Measuring, ${count(elapsed)}s` : form.label}
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
