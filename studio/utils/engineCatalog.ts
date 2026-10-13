/**
 * The Engine's catalog, as a surface that binds something to a table needs to
 * offer it.
 *
 * Two representations of one fact exist and they do not agree: the Engine
 * holds catalog *definitions* (everything registered against it, benchmark
 * scratch included), while the platform holds catalog *source records* — the
 * catalogs an operator registered on purpose, with a lifecycle. Offering raw
 * Engine definitions would offer scratch; offering source records alone would
 * leave us without the Engine ids the schema and table listings are keyed by.
 *
 * So `fetchRegisteredCatalogs` derives the offer from the source records and
 * resolves each one to its Engine definition by name. A source record with no
 * definition is reported separately rather than silently dropped: it means the
 * catalog was registered but never synchronized, which is the one thing a
 * reader who cannot find their table needs to be told.
 */
import { API_BASE } from "../config";
import { msalFetch } from "./msalFetch";

export interface EngineCatalog {
  /** The Engine's durable id — what the schema listing is keyed by. */
  id: string;
  name: string;
  lifecycle: string;
  /** The platform's description of the catalog source, when it carries one. */
  description: string | null;
  /** `local` · `adls_gen2` · `s3`, from the source record. */
  storageType: string | null;
  /** `parquet` · `delta` · `iceberg`, from the source record. */
  dataFormat: string | null;
}

export interface EngineSchema {
  id: string;
  catalog_id: string;
  name: string;
  lifecycle: string;
}

export interface EngineColumn {
  name: string;
  /** Arrow type as the Engine serializes it: a name, or a one-key object. */
  data_type: unknown;
  nullable?: boolean;
}

export interface EngineTable {
  id: string;
  schema_id: string;
  name: string;
  lifecycle: string;
  location?: string;
  format?: string;
  columns?: EngineColumn[];
  /** The shape a cube is built over; absent unless declared. */
  shape?: {
    dimensions?: { name: string }[];
    measures?: { column: string; aggregates?: string[] }[];
    time?: { column?: string } | null;
  };
}

export interface EngineMeasurement {
  tableId: string;
  state: "measured" | "unmeasured" | "unreadable";
  rows?: number | null;
  bytes?: number | null;
  files?: number | null;
  error?: string | null;
}

export interface RegisteredCatalogs {
  /** Active catalog sources that resolve to an active Engine definition. */
  catalogs: EngineCatalog[];
  /**
   * Registered catalogs that resolved to no active Engine definition, by name.
   * Two causes look identical from here and both belong in the message: the
   * registration was never synchronized to the Engine, or the definition read
   * answered without it.
   */
  unsynchronized: string[];
}

interface EngineSourceRow {
  id: string | number;
  name: string;
  /** The catalog name, as the platform's registry reports it. */
  catalog: string | null;
}

const ACTIVE = "Active";

function reason(status: number, body: unknown): string {
  const detail = (body as { detail?: unknown } | null)?.detail;
  if (typeof detail === "string" && detail.trim()) return detail;
  if (detail && typeof detail === "object") {
    const message = (detail as { message?: unknown }).message;
    if (typeof message === "string" && message.trim()) return message;
  }
  const error = (body as { error?: unknown } | null)?.error;
  if (typeof error === "string" && error.trim()) return error;
  return `The platform answered ${status}.`;
}

async function get<T>(path: string): Promise<T> {
  const res = await msalFetch(`${API_BASE}/api/v1${path}`);
  const body: unknown = await res.json().catch(() => null);
  if (!res.ok) throw new Error(reason(res.status, body));
  return body as T;
}

const enc = encodeURIComponent;

/**
 * The catalogs a dataset may be bound to.
 *
 * The registry read is `/lab/engine/sources` — the same one SQL Lab honours,
 * deliberately. It is the only read that applies the caller's catalog grants,
 * so a reader is never offered a catalog they cannot query; and it resolves
 * the product catalog's several names server-side, which a comparison here
 * cannot. An earlier version of this joined `/catalog-sources` to definitions
 * on an exact `engine_catalog` string, which was neither grant-filtered nor
 * able to tell that the Engine's `kaveon` and the registry's `KaveonDB` are
 * one catalog — it reported a registered catalog as unsynchronized.
 *
 * Engine definitions are still read, for the one thing only they carry: the
 * catalog id a schema listing is keyed by. A catalog must appear in both to be
 * offered — one the registry does not list cannot be queried, and one the
 * Engine does not define cannot be opened. Benchmark scratch is excluded by
 * having no registration, never by matching its name.
 *
 * Names are matched without regard to case, for the same reason the API does:
 * a capitalization difference is not a different catalog.
 */
export async function fetchRegisteredCatalogs(): Promise<RegisteredCatalogs> {
  const [registry, definitions] = await Promise.all([
    get<{ sources?: EngineSourceRow[] }>("/lab/engine/sources"),
    get<{ definitions?: EngineCatalog[] }>("/engine/catalog/definitions"),
  ]);
  const byName = new Map<string, EngineCatalog>();
  for (const definition of definitions.definitions ?? []) {
    if (definition && typeof definition.name === "string") {
      byName.set(definition.name.trim().toLowerCase(), definition);
    }
  }
  // The product catalog is defined by the Engine under a different name than
  // the registry reports, so the two spellings resolve to one definition.
  const PRODUCT_ALIASES = ["kaveon", "kaveondb"];
  const productDefinition = PRODUCT_ALIASES.map((alias) => byName.get(alias)).find(Boolean);

  const catalogs: EngineCatalog[] = [];
  const unsynchronized: string[] = [];
  for (const source of registry.sources ?? []) {
    const named = (source?.catalog ?? "").trim();
    if (!named) continue;
    const key = named.toLowerCase();
    const definition = byName.get(key)
      ?? (PRODUCT_ALIASES.includes(key) ? productDefinition : undefined);
    if (!definition || definition.lifecycle !== ACTIVE) {
      unsynchronized.push(source.name || named);
      continue;
    }
    if (catalogs.some((existing) => existing.id === definition.id)) continue;
    catalogs.push({
      id: definition.id,
      name: named,
      lifecycle: definition.lifecycle,
      description: null,
      storageType: null,
      dataFormat: null,
    });
  }
  catalogs.sort((a, b) => a.name.localeCompare(b.name));
  return { catalogs, unsynchronized };
}

/** Active schemas of one catalog. The Engine publishes only active definitions. */
export async function fetchCatalogSchemas(catalogId: string): Promise<EngineSchema[]> {
  const body = await get<{ schemas?: EngineSchema[] }>(`/engine/catalog/definitions/${enc(catalogId)}/schemas`);
  return (body.schemas ?? [])
    .filter(schema => schema && schema.lifecycle === ACTIVE)
    .sort((a, b) => a.name.localeCompare(b.name));
}

/** Active tables of one schema, with their columns and declared shape. */
export async function fetchSchemaTables(schemaId: string): Promise<EngineTable[]> {
  const body = await get<{ tables?: EngineTable[] }>(`/engine/catalog/schemas/${enc(schemaId)}/tables`);
  return (body.tables ?? [])
    .filter(table => table && table.lifecycle === ACTIVE)
    .sort((a, b) => a.name.localeCompare(b.name));
}

/**
 * What the Engine has measured about every table in one schema. The platform
 * fans this out server-side and caches it, so it is one request rather than
 * two per table.
 */
export async function fetchSchemaInventory(schemaId: string): Promise<EngineMeasurement[]> {
  const body = await get<{ measurements?: EngineMeasurement[] }>(`/engine/catalog/schemas/${enc(schemaId)}/inventory`);
  return body.measurements ?? [];
}

/** One table by the id a dataset is bound with. */
export async function fetchEngineTable(tableId: string): Promise<EngineTable> {
  const body = await get<{ table?: EngineTable }>(`/engine/catalog/tables/${enc(tableId)}`);
  if (!body.table) throw new Error("The Engine has no definition for this table.");
  return body.table;
}

// Arrow type names as the Engine's catalog serializes them → the platform's
// column type. The same mapping the API applies when it binds a dataset
// (`services/engine_datasets.sql_type`), so the type shown here is the type
// the dataset will carry.
const ARROW_TO_SQL: Record<string, string> = {
  Boolean: "boolean",
  Int8: "tinyint", Int16: "smallint", Int32: "integer", Int64: "bigint",
  UInt8: "tinyint", UInt16: "smallint", UInt32: "integer", UInt64: "bigint",
  Float16: "real", Float32: "real", Float64: "double",
  Utf8: "varchar", LargeUtf8: "varchar",
  Binary: "varbinary", LargeBinary: "varbinary",
  Date32: "date", Date64: "date",
};

export function columnType(arrow: unknown): string {
  if (typeof arrow === "string") return ARROW_TO_SQL[arrow] ?? arrow.toLowerCase();
  if (arrow && typeof arrow === "object") {
    const entries = Object.entries(arrow as Record<string, unknown>);
    if (entries.length === 1) {
      const [name, args] = entries[0];
      if (name === "Timestamp") return "timestamp";
      if ((name === "Decimal128" || name === "Decimal256") && Array.isArray(args) && args.length === 2) {
        return `decimal(${args[0]}, ${args[1]})`;
      }
      if (name === "Dictionary" && Array.isArray(args) && args.length === 2) return columnType(args[1]);
      return name.toLowerCase();
    }
  }
  return "unknown";
}

function baseType(sqlType: string): string {
  return sqlType.split("(", 1)[0].trim().toLowerCase();
}

export function isTemporal(sqlType: string): boolean {
  const base = baseType(sqlType);
  return base === "date" || base === "timestamp";
}

/** The table's date and timestamp columns, in definition order. */
export function temporalColumns(table: EngineTable | null): { name: string; type: string }[] {
  if (!table?.columns) return [];
  return table.columns
    .map(column => ({ name: column.name, type: columnType(column.data_type) }))
    .filter(column => isTemporal(column.type));
}

/**
 * The date column the API would derive for this table if the caller sends
 * none: the column a declared shape names as its time column, otherwise the
 * first date or timestamp column. Mirrors `engine_datasets.semantics`.
 */
export function derivedDateColumn(table: EngineTable | null): string | null {
  if (!table) return null;
  const declared = table.shape?.time?.column;
  if (typeof declared === "string" && declared.trim()) return declared;
  if (table.shape) return null;
  return temporalColumns(table)[0]?.name ?? null;
}

const COUNT = new Intl.NumberFormat("en-US");

export function formatRows(rows: number | null | undefined): string | null {
  return typeof rows === "number" && Number.isFinite(rows) ? COUNT.format(rows) : null;
}

export function formatBytes(bytes: number | null | undefined): string | null {
  if (typeof bytes !== "number" || !Number.isFinite(bytes)) return null;
  const units = ["B", "KB", "MB", "GB", "TB", "PB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value < 10 && unit > 0 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
}
