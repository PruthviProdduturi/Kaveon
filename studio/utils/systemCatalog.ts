import { API_BASE } from "../config";
import { msalFetch } from "./msalFetch";

/**
 * KaveonDB — the Engine's built-in catalog for the platform's own records.
 *
 * Two names, one catalog. `kaveon` is the SQL identifier the Engine resolves
 * and the only name that belongs in a statement; KaveonDB is what the product
 * calls it, and appears in headings and labels only. Nothing here ever writes
 * the product name into an identifier, and every table it lists carries its
 * real qualified name so the one a reader copies is the one that resolves.
 *
 * Its `product` schema is not a schema the query planner knows: the Engine's
 * parser recognises `kaveon.product.<table>` as the facade for its typed
 * record transactions and routes those statements to the product manifest, so
 * the tables are written and read through that boundary and cannot be
 * scanned. SQL Lab therefore lists them without offering to query them.
 */
export const SYSTEM_CATALOG_IDENTIFIER = "kaveon";
export const SYSTEM_CATALOG_SCHEMA = "product";
export const SYSTEM_CATALOG_LABEL = "KaveonDB";

/** Whether a catalog name from the Engine is the platform's own catalog. */
export function isSystemCatalog(catalog: string | null | undefined): boolean {
  return typeof catalog === "string"
    && catalog.toLowerCase() === SYSTEM_CATALOG_IDENTIFIER;
}

/** The name to show a reader. Every other catalog is shown as it is named. */
export function catalogLabel(catalog: string): string {
  return isSystemCatalog(catalog) ? SYSTEM_CATALOG_LABEL : catalog;
}

/** One record family in the `product` schema, as the platform reports it. */
export interface SystemTable {
  /** The record kind the platform's API addresses this family by. */
  kind: string;
  /** The table name inside the `product` schema. */
  table: string;
  /** The real qualified identifier — `kaveon.product.<table>`. */
  identifier: string;
  /** Records counted, or null when the reading did not count them. */
  records: number | null;
  /** True when the family holds more than `records`, which is then a floor. */
  truncated: boolean;
  snapshotId: string | null;
  /** The Engine's message for a family it refused; null when it answered. */
  error: string | null;
}

/**
 * Where the deployment keeps all of this. Read from the server's own
 * configuration, never assumed: `configured` is false when the deployment has
 * not said, and the UI reports that rather than naming a location it cannot
 * stand behind. `durable` is false for a host directory, which does not
 * survive the container that writes it.
 */
export interface SystemStore {
  configured: boolean;
  mode: "local" | "adls" | null;
  durable: boolean | null;
  account: string | null;
  container: string | null;
  prefix: string | null;
  location: string | null;
}

export interface SystemCatalogReading {
  catalog: { identifier: string; schema: string };
  storage: SystemStore;
  /** Whether this reading counted records, or only listed the families. */
  counted: boolean;
  /** The committed generation read, and whether all of it came from one. */
  snapshot: { id: string | null; consistent: boolean };
  tables: SystemTable[];
}

/**
 * What each family holds, in one line. These are descriptions of the records
 * themselves, not of who may read them: an ordinary person's own history,
 * pins and recents live here and are read and written on their behalf every
 * day. What is administrator-only is browsing the tables directly.
 */
export const SYSTEM_TABLE_HOLDS: Record<string, string> = {
  datasets: "Every registered dataset: the table it reads and the semantic columns, dimensions and metrics declared over it.",
  charts: "Each saved chart: its type, its encodings, its filters and the dataset it reads.",
  dashboards: "Each dashboard: its layout, its filters and the charts placed on it.",
  saved_queries: "Statements saved in SQL Lab, with the catalog and schema they were written against.",
  sources: "Registered data sources and the catalogs behind them, holding credential references rather than credentials.",
  dlm_definitions: "One context specification per dataset: the aliases, breakdowns and additivity rules the DLM routes on.",
  dlm_runs: "Each DLM build: what it generated, when it ran and over how many rows.",
  chat_sessions: "Each Ask conversation and the dataset it is scoped to.",
  chat_messages: "The turns inside those conversations, with the statement each answer was derived from.",
  favorites: "Per-person pins across datasets, charts and dashboards.",
  user_themes: "Per-person interface preferences.",
  user_recents: "What each person opened last, by record kind.",
  query_history: "Every statement run through the platform: its text, its elapsed time and its outcome.",
  activity: "The audit ledger: which record changed, how, by whom and when.",
};

export class SystemCatalogError extends Error {
  constructor(public status: number, message: string) { super(message); }
}

/**
 * Read the system catalog. The structure is free; `counts` adds a bounded
 * record count per family, which the Engine answers by reading the records
 * themselves and so is asked for separately once the schema is on screen.
 */
export async function fetchSystemCatalog(
  { counts = false }: { counts?: boolean } = {},
): Promise<SystemCatalogReading> {
  const response = await msalFetch(
    `${API_BASE}/api/v1/engine/console/system-catalog${counts ? "?counts=true" : ""}`);
  if (!response.ok) {
    const body = (await response.json().catch(() => null)) as { detail?: unknown } | null;
    const detail = body?.detail;
    const message = typeof detail === "string" ? detail
      : detail && typeof detail === "object" && typeof (detail as { message?: unknown }).message === "string"
        ? (detail as { message: string }).message
        : response.status === 403
          ? `Browsing ${SYSTEM_CATALOG_LABEL}'s tables requires the administrator role.`
          : `${SYSTEM_CATALOG_LABEL} could not be read (${response.status}).`;
    throw new SystemCatalogError(response.status, message);
  }
  return (await response.json()) as SystemCatalogReading;
}

/** An exact count with thousands separators, or a floor when it was bounded. */
export function recordCount(table: SystemTable): string {
  if (typeof table.records !== "number") return "—";
  return table.truncated ? `${table.records.toLocaleString()}+` : table.records.toLocaleString();
}
