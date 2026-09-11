import { API_BASE } from "../../config";
import { msalFetch } from "../../utils/msalFetch";

// The Catalog reads the same endpoints as the SQL Lab tree, so the two can
// never disagree about what exists. Only the table definition is Catalog's own.

export interface EngineSource { id: string; name: string; catalog: string }
export interface ColumnDef { name: string; dataType: string; isNullable: boolean }
export interface TableDef {
  catalog: string; schema: string; name: string;
  location: string | null;
  access: "Shortcut" | "Optimized" | null;
  format: "Parquet" | "Delta" | "Iceberg" | null;
  revision: number | null;
  lifecycle: string | null;
  columns: ColumnDef[];
}
export interface Sample { columns: string[]; rows: unknown[][]; executionTime: number }

export class CatalogError extends Error {
  constructor(public status: number, message: string) { super(message); }
}

async function get<T>(path: string): Promise<T> {
  const res = await msalFetch(`${API_BASE}/api/v1${path}`);
  if (!res.ok) throw new CatalogError(res.status, await reason(res));
  return res.json() as Promise<T>;
}

async function reason(res: Response): Promise<string> {
  if (res.status === 503) return "KaveonDB is not configured on this server.";
  if (res.status === 502) return "KaveonDB did not answer. It may be restarting or unreachable from the server.";
  if (res.status === 404) return "KaveonDB no longer has this definition.";
  if (res.status === 403) return "Your role does not include this action.";
  try { const body = await res.json(); if (typeof body?.detail === "string") return body.detail; } catch { /* fall through */ }
  return `The server returned ${res.status}.`;
}

export const fetchSources = async () => (await get<{ sources: EngineSource[] }>("/lab/engine/sources")).sources;
export const fetchSchemas = async (source: string) => (await get<{ schemas: string[] }>(`/lab/engine/${enc(source)}/schemas`)).schemas;
export const fetchTables = async (source: string, schema: string) =>
  (await get<{ tables: string[] }>(`/lab/engine/${enc(source)}/schemas/${enc(schema)}/tables`)).tables;
export const fetchTable = async (source: string, schema: string, table: string) =>
  (await get<{ table: TableDef }>(`/catalog/${enc(source)}/schemas/${enc(schema)}/tables/${enc(table)}`)).table;

export async function fetchSample(source: string, schema: string, table: string, limit = 20): Promise<Sample> {
  const res = await msalFetch(`${API_BASE}/api/v1/lab/query`, {
    method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ query: sampleSql(schema, table, limit), engineSourceId: source, engineSchema: schema }),
  });
  if (!res.ok) throw new CatalogError(res.status, await reason(res));
  return res.json() as Promise<Sample>;
}

export function enc(v: string) { return encodeURIComponent(v); }
export function quoteIdent(v: string) { return `"${v.replace(/"/g, '""')}"`; }
export function sampleSql(schema: string, table: string, limit: number) {
  return `SELECT * FROM ${quoteIdent(schema)}.${quoteIdent(table)} LIMIT ${limit}`;
}
export function labHref(catalog: string, schema: string, table: string) {
  const sql = `SELECT *\nFROM ${quoteIdent(catalog)}.${quoteIdent(schema)}.${quoteIdent(table)}\nLIMIT 100`;
  return `/lab?query=${encodeURIComponent(sql)}`;
}
export function shortLocation(location: string): { host: string; path: string } {
  const m = location.match(/^([a-z0-9+.-]+:\/\/[^/]+)(\/.*)?$/i);
  return m ? { host: m[1], path: m[2] || "/" } : { host: "", path: location };
}
