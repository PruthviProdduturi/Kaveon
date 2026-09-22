import { API_BASE } from "../../config";
import { msalFetch } from "../../utils/msalFetch";

// The Catalog reads the same endpoints as the SQL Lab tree, so the two can
// never disagree about what exists. Only the table definition is Catalog's own.

export interface EngineSource { id: string; name: string; catalog: string }
export interface ColumnDef { name: string; dataType: string; isNullable: boolean }
export interface TableDef {
  id: string | null;         // the Engine's durable table id; with `revision`, what Remove needs
  schemaId: string | null;
  catalog: string; schema: string; name: string;
  location: string | null;
  access: "Shortcut" | "Optimized" | null;
  format: "Parquet" | "Delta" | "Iceberg" | null;
  revision: number | null;
  lifecycle: string | null;
  columns: ColumnDef[];
  rowCount: number | null;   // exact, from footer statistics on KaveonDB; null when not yet counted
}
export interface Sample { columns: string[]; rows: unknown[][]; executionTime: number }
export interface Usage {
  datasets: { id: number; name: string; visibility: string }[];
  charts: { id: number; name: string; datasetId: number }[];
  dashboards: { id: string | number; name: string; slug?: string | null }[];
  dlm: { datasetId: string; status: string | null; builtAt: string | null; rowCount: number | null; rowCountSource: string | null }[];
}
export interface TableStatistic { table: string; row_count: number; current: boolean }

export class CatalogError extends Error {
  constructor(public status: number, message: string) { super(message); }
}

async function get<T>(path: string): Promise<T> {
  const res = await msalFetch(`${API_BASE}/api/v1${path}`);
  if (!res.ok) throw new CatalogError(res.status, await reason(res));
  return res.json() as Promise<T>;
}

/** The API's error detail: a string, a {code, message} record, or FastAPI's validation list. */
function detailText(detail: unknown): string | null {
  if (typeof detail === "string") return detail;
  if (Array.isArray(detail)) {
    const parts = detail
      .map(item => (item && typeof item === "object" && typeof (item as { msg?: unknown }).msg === "string")
        ? (item as { msg: string }).msg.replace(/^Value error, /, "") : null)
      .filter((m): m is string => !!m);
    return parts.length ? parts.join(" ") : null;
  }
  if (detail && typeof detail === "object" && typeof (detail as { message?: unknown }).message === "string") {
    return (detail as { message: string }).message;
  }
  return null;
}

async function reason(res: Response, body?: unknown): Promise<string> {
  const known = body === undefined ? await res.json().catch(() => null) : body;
  const text = detailText((known as { detail?: unknown } | null)?.detail);
  if (res.status === 503) return text ?? "KaveonDB is not configured on this server.";
  if (res.status === 502) return text ?? "KaveonDB did not answer. It may be restarting or unreachable from the server.";
  if (res.status === 404) return text ?? "KaveonDB no longer has this definition.";
  if (res.status === 403) return "Your role does not include this action.";
  return text ?? `The server returned ${res.status}.`;
}

export const fetchSources = async () => (await get<{ sources: EngineSource[] }>("/lab/engine/sources")).sources;
export const fetchSchemas = async (source: string) => (await get<{ schemas: string[] }>(`/lab/engine/${enc(source)}/schemas`)).schemas;
export const fetchTables = async (source: string, schema: string) =>
  (await get<{ tables: string[] }>(`/lab/engine/${enc(source)}/schemas/${enc(schema)}/tables`)).tables;
export const fetchTable = async (source: string, schema: string, table: string) =>
  (await get<{ table: TableDef }>(`/catalog/${enc(source)}/schemas/${enc(schema)}/tables/${enc(table)}`)).table;
export const fetchUsage = (source: string, schema: string, table: string) =>
  get<Usage>(`/catalog/${enc(source)}/schemas/${enc(schema)}/tables/${enc(table)}/usage`);

/** Administrators only: KaveonDB's bounded statistics diagnostic, filtered to one table. */
export async function fetchStatistic(fullName: string): Promise<TableStatistic | null> {
  const res = await msalFetch(`${API_BASE}/api/v1/engine/console/statistics`);
  if (!res.ok) return null;
  const body = (await res.json()) as { statistics?: TableStatistic[] };
  return body.statistics?.find(s => s.table === fullName) ?? null;
}

export async function fetchSample(source: string, schema: string, table: string, limit = 20): Promise<Sample> {
  const res = await msalFetch(`${API_BASE}/api/v1/lab/query`, {
    method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ query: sampleSql(schema, table, limit), engineSourceId: source, engineSchema: schema }),
  });
  if (!res.ok) throw new CatalogError(res.status, await reason(res));
  return res.json() as Promise<Sample>;
}

export function enc(v: string) { return encodeURIComponent(v); }
// KaveonDB's parser keeps ANSI quotes as part of an identifier, so a plain
// identifier is written bare; only a name that needs quoting gets quotes.
export function quoteIdent(v: string) { return /^[A-Za-z_][A-Za-z0-9_]*$/.test(v) ? v : `"${v.replace(/"/g, '""')}"`; }
export function sampleSql(schema: string, table: string, limit: number) {
  return `SELECT * FROM ${quoteIdent(schema)}.${quoteIdent(table)} LIMIT ${limit}`;
}
export function labHref(catalog: string, schema: string, table: string) {
  const sql = `SELECT *\nFROM ${quoteIdent(catalog)}.${quoteIdent(schema)}.${quoteIdent(table)}\nLIMIT 100`;
  return `/lab?catalog=${enc(catalog)}&schema=${enc(schema)}&name=${enc(table)}&query=${encodeURIComponent(sql)}`;
}
// ── Reading the measurements ─────────────────────────────────────────────────
// Numbers a reader has to trust are never rounded away: a row count is written
// in full, and a byte total in the binary units storage is actually billed and
// listed in. Nothing here invents a value for a fact that was not measured.

/** An exact count with thousands separators; an em dash when nothing was counted. */
export const count = (value: number | null | undefined) =>
  typeof value === "number" ? value.toLocaleString() : "—";

/** Bytes as stored, in binary units: 2.23 KiB, 68.4 MiB, 1.21 GiB. */
export function bytes(value: number | null | undefined): string {
  if (typeof value !== "number") return "—";
  if (value < 1024) return `${value} B`;
  const units = ["KiB", "MiB", "GiB", "TiB", "PiB"];
  let size = value / 1024, unit = 0;
  while (size >= 1024 && unit < units.length - 1) { size /= 1024; unit += 1; }
  return `${size >= 100 ? Math.round(size) : size.toFixed(size >= 10 ? 1 : 2)} ${units[unit]}`;
}

/** How long ago, in the coarsest unit that still says something: 4 min, 6 h, 12 d. */
export function since(ms: number | null | undefined): string {
  if (typeof ms !== "number" || !Number.isFinite(ms)) return "—";
  const seconds = Math.max(0, Math.round((Date.now() - ms) / 1000));
  if (seconds < 90) return "just now";
  const minutes = Math.round(seconds / 60);
  if (minutes < 90) return `${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 36) return `${hours} h ago`;
  const days = Math.round(hours / 24);
  if (days < 45) return `${days} d ago`;
  const months = Math.round(days / 30);
  return months < 24 ? `${months} mo ago` : `${Math.round(months / 12)} y ago`;
}

export const exactTime = (ms: number | null | undefined) =>
  typeof ms === "number" && Number.isFinite(ms) ? new Date(ms).toLocaleString() : "";

/** The Engine's own name for a version: `delta v12`, `iceberg snapshot 8842`, `12 files`, `single file`. */
export function versionLabel(version: SourceVersion | null | undefined): string {
  if (!version) return "—";
  switch (version.kind) {
    case "delta_version": return `delta v${version.version}`;
    case "iceberg_snapshot": return version.snapshot_id != null ? `iceberg snapshot ${version.snapshot_id}` : "iceberg";
    case "listing": return `${version.files.toLocaleString()} file${version.files === 1 ? "" : "s"}`;
    case "file": return "single file";
    default: return "—";
  }
}

/** The first twelve characters of the version digest — what the Engine prints. */
export const versionDigest = (version: SourceVersion | null | undefined) =>
  version?.identity_sha256 ? version.identity_sha256.slice(0, 12) : "";

/** The declared format, as the catalog holds it. */
export const formatLabel = (format: TableFormat | string | null) => format || "—";

/**
 * Whether a Parquet table is one file or a directory of them — a distinction
 * the source version makes and the definition does not, and the difference
 * between a table that grows by appending files and one that is rewritten.
 * Empty for Delta and Iceberg, whose logs already say it.
 */
export function formatKind(format: TableFormat | string | null, version?: SourceVersion | null): string {
  if (format !== "Parquet" || !version) return "";
  if (version.kind === "file") return "single file";
  if (version.kind === "listing") return "directory";
  return "";
}

export function shortLocation(location: string): { host: string; path: string } {
  const m = location.match(/^([a-z0-9+.-]+:\/\/[^/]+)(\/.*)?$/i);
  return m ? { host: m[1], path: m[2] || "/" } : { host: "", path: location };
}

// ── Registration ─────────────────────────────────────────────────────────────
// Adding a schema or a table speaks to /api/v1/engine/catalog, which holds the
// Engine's durable definitions: stable ids, revisions, Draft → Active. Studio
// works in names; the ids are resolved here and never typed.

export type TableFormat = "Delta" | "Iceberg" | "Parquet";
export interface CatalogDefinition {
  id: string; name: string; revision: number; lifecycle: string;
  /** The Engine's storage record, keyed by kind: {AdlsGen2: {account, container, root_path}} | {Local: {base_path}} | {S3: {bucket, region, prefix}}. */
  storage?: Record<string, Record<string, string>>;
}

/** Where a table location is relative to, from the catalog's storage record; "" when unknown. */
export function storageRoot(def: CatalogDefinition | null | undefined): string {
  const storage = def?.storage;
  if (!storage) return "";
  const join = (...parts: (string | undefined)[]) => parts.filter(p => p && p.trim()).map(p => p!.replace(/^\/+|\/+$/g, "")).filter(Boolean).join("/");
  if (storage.AdlsGen2) return join(storage.AdlsGen2.container, storage.AdlsGen2.root_path);
  if (storage.Local) return join(storage.Local.base_path);
  if (storage.S3) return join(storage.S3.bucket, storage.S3.prefix);
  return "";
}
export interface SchemaDefinition { id: string; catalog_id: string; name: string; revision: number; lifecycle: string }
export interface EngineColumn { name: string; data_type: unknown; nullable: boolean }
export interface EngineTable {
  id: string; schema_id: string; name: string; revision: number; lifecycle: string;
  location: string; access: string; format: TableFormat; columns: EngineColumn[];
  /** Columns read from `key=value` path segments; absent unless the table is a partitioned directory. */
  partitions?: { name: string; data_type?: unknown }[];
  /** Clustering and Bloom columns; absent unless declared. */
  layout?: { clustered_by?: string[]; bloom?: string[] };
  /** The shape a cube is built over; absent unless declared. */
  shape?: { dimensions?: { name: string; cap?: number }[]; measures?: { column: string; aggregates: string[] }[]; time?: unknown };
}
export interface ColumnInput { name: string; type: string; nullable: boolean }
export interface Probe { rowCount: number; elapsedMs: number | null; queryId: string | null }

/** A table the Engine could not read: the definition was taken back out and the storage error kept verbatim. */
export class TableUnreadableError extends CatalogError {
  constructor(message: string, public engineCode: string | null, public removed: boolean) { super(422, message); }
}

async function send<T>(path: string, init: RequestInit): Promise<T> {
  const res = await msalFetch(`${API_BASE}/api/v1${path}`, init);
  const body: unknown = res.status === 204 ? null : await res.json().catch(() => null);
  if (res.ok) return body as T;
  const detail = (body as { detail?: unknown } | null)?.detail;
  if (res.status === 422 && detail && typeof detail === "object" && (detail as { code?: string }).code === "table_unreadable") {
    const d = detail as { message: string; engineCode?: string | null; removed?: boolean };
    throw new TableUnreadableError(d.message, d.engineCode ?? null, d.removed !== false);
  }
  throw new CatalogError(res.status, await reason(res, body));
}

const json = (method: string, body: unknown, headers: Record<string, string> = {}): RequestInit =>
  ({ method, headers: { "Content-Type": "application/json", ...headers }, body: JSON.stringify(body) });

export const fetchDefinitions = async () =>
  (await get<{ definitions: CatalogDefinition[] }>("/engine/catalog/definitions")).definitions;
export const fetchSchemaDefinitions = async (catalogId: string) =>
  (await get<{ schemas: SchemaDefinition[] }>(`/engine/catalog/definitions/${enc(catalogId)}/schemas`)).schemas;

export const fetchTableDefinitions = async (schemaId: string) =>
  (await get<{ tables: EngineTable[] }>(`/engine/catalog/schemas/${enc(schemaId)}/tables`)).tables;

// ── Inventory ────────────────────────────────────────────────────────────────
// What the Engine has measured about each table in a schema, read in one call
// per schema rather than two per table. Definitions arrive first and draw the
// rows; measurements fill the cells those rows already reserved.

/** How the source is versioned, and therefore how a change to it is noticed. */
export type SourceVersion =
  | { kind: "delta_version"; version: number; identity_sha256: string }
  | { kind: "iceberg_snapshot"; snapshot_id: number | null; identity_sha256: string }
  | { kind: "listing"; files: number; identity_sha256: string }
  | { kind: "file"; identity_sha256: string };

export interface Measurement {
  tableId: string;
  /** measured: statistics on record · unmeasured: never analyzed · unreadable: the location did not answer. */
  state: "measured" | "unmeasured" | "unreadable";
  error?: string | null;
  sourceVersion?: SourceVersion | null;
  currentSourceVersion?: SourceVersion | null;
  observedAtMs?: number | null;
  stale?: boolean;
  computedAtMs?: number | null;
  depth?: "metadata" | "full" | null;
  rows?: number | null;
  bytes?: number | null;
  files?: number | null;
  rowGroups?: number | null;
  uncompressedBytes?: number | null;
  lastModifiedMs?: number | null;
  partitionColumns?: string[];
  measuredColumns?: number | null;
}

export const fetchInventory = async (schemaId: string, refresh = false) =>
  (await get<{ measurements: Measurement[] }>(
    `/engine/catalog/schemas/${enc(schemaId)}/inventory${refresh ? "?refresh=true" : ""}`)).measurements;

/** How deeply to measure a table; nothing set reads metadata only. */
export interface AnalyzeDepth { sketches?: boolean; distinct?: boolean; cube?: boolean }
export interface AnalyzeResult {
  statement: string;
  result: { table?: string; row_count?: number; distinct_columns?: number; cube_cells?: number | null };
}
export const analyzeTable = (tableId: string, depth: AnalyzeDepth) =>
  send<AnalyzeResult>(`/engine/catalog/tables/${enc(tableId)}/analyze`, json("POST", depth));

export const createSchema = async (catalogId: string, name: string) =>
  (await send<{ schema: SchemaDefinition }>(`/engine/catalog/definitions/${enc(catalogId)}/schemas`, json("POST", { name }))).schema;

export const createTable = async (input: {
  schemaId: string; name: string; location: string; format: TableFormat; columns: ColumnInput[];
}) => send<{ table: EngineTable; probe: Probe | null }>("/engine/catalog/tables", json("POST", {
  schema_id: input.schemaId, name: input.name, location: input.location, format: input.format,
  columns: input.columns.map(c => ({ name: c.name, type: c.type, nullable: c.nullable })),
}));

export const deleteTable = (tableId: string, revision: number) =>
  send<null>(`/engine/catalog/tables/${enc(tableId)}`, { method: "DELETE", headers: { "If-Match": String(revision) } });

/**
 * Columns as typed, one per line: `name type`, optionally `not null`.
 * Types are what a Trino user writes (bigint, varchar, double, date,
 * timestamp, decimal(18, 2)); the API maps them to the Engine's Arrow types.
 */
export function parseColumns(text: string): { columns: ColumnInput[]; error: string | null } {
  const columns: ColumnInput[] = [];
  const seen = new Set<string>();
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim().replace(/,\s*$/, "");
    if (!line) continue;
    const m = line.match(/^([A-Za-z_][A-Za-z0-9_]*)\s+(.+?)(\s+not\s+null|\s+nullable)?$/i);
    if (!m) return { columns, error: `Cannot read "${line}". Write a name and a type, as in order_id bigint.` };
    const name = m[1], type = m[2].trim();
    if (seen.has(name.toLowerCase())) return { columns, error: `Column ${name} is listed twice.` };
    seen.add(name.toLowerCase());
    columns.push({ name, type, nullable: !/not\s+null/i.test(m[3] ?? "") });
  }
  return { columns, error: null };
}

export const COLUMN_TYPES = "bigint, integer, smallint, tinyint, double, real, boolean, varchar, varbinary, date, timestamp, timestamp(3), decimal(p, s)";
