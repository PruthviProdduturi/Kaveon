import { API_BASE } from "../../config";
import { msalFetch } from "../../utils/msalFetch";

// ── Engine record shapes ──────────────────────────────────────────────────────
// These mirror the coordinator's /v1 serialization exactly. Nothing is derived
// client-side that the Engine did not measure; absent values render as absent.

export type QueryState = "RUNNING" | "FINISHED" | "FAILED";

export interface ClusterNode {
  node_id?: string;
  version?: string;
  uptime_secs?: number;
  memory_rss_bytes?: number;
}

export interface Cluster {
  environment: string;
  coordinator: ClusterNode & { version: string; uptime_secs: number };
  workers: ClusterNode[];
  active_workers: number;
  total_nodes: number;
}

export interface ColumnInfo { name: string; type: string }

export interface QueryTimings {
  analysis_us?: number | null;
  planning_us?: number | null;
  execution_us?: number | null;
  result_serialization_us?: number | null;
}

export interface ScanTelemetry {
  files_considered: number; files_opened: number;
  row_groups_considered: number; row_groups_read: number; row_groups_pruned: number;
  rows_selected: number; rows_emitted: number;
  compressed_bytes_selected: number;
  compressed_bytes_per_second?: number; rows_per_second?: number;
  snapshot_ns?: number; footer_ns?: number; read_ns?: number;
}

export interface TaskTelemetry {
  task_id: string; node_id: string; partition_index: number;
  elapsed_us: number; output_rows: number; output_batches: number; output_bytes: number;
}

export interface StageTelemetry {
  stage_id: number; state: string; task_count: number; completed_tasks: number;
  elapsed_us: number; tasks: TaskTelemetry[];
}

export interface QueryContext {
  engine_version?: string; environment?: string;
  principal?: string | null; user?: string | null;
  source?: string | null; client?: string | null;
  catalog?: string; schema?: string;
  time_zone?: string | null; client_address?: string | null;
  client_tags?: string[]; result_delivery?: string | null;
  catalog_snapshot_id?: string;
}

export interface PlanNode {
  id?: string; operator?: string; phase?: string;
  attributes?: Record<string, unknown>; children?: PlanNode[];
}

export interface QueryRecord {
  id: string;
  sql: string;
  state: QueryState;
  columns: ColumnInfo[];
  rows: unknown[][];
  rows_are_preview: boolean;
  scan_metrics_complete: boolean;
  error?: string | null;
  elapsed_ms: number;
  submitted_at_ms: number;
  completed_at_ms: number;
  timings: QueryTimings;
  plan?: { logical?: PlanNode | string | null };
  scans: ScanTelemetry[];
  stages: StageTelemetry[];
  context: QueryContext;
}

// ── Transport ─────────────────────────────────────────────────────────────────

export class EngineUnavailable extends Error {
  constructor(public status: number, message: string) { super(message); }
}

async function get<T>(path: string): Promise<T> {
  const res = await msalFetch(`${API_BASE}/api/v1/engine/console${path}`);
  if (res.status === 503) throw new EngineUnavailable(503, "KaveonDB is not configured on this server.");
  if (res.status === 502) throw new EngineUnavailable(502, "KaveonDB did not answer. It may be restarting or unreachable from the server.");
  if (res.status === 404) throw new EngineUnavailable(404, "This query is not in KaveonDB's current history, or you do not have access to it.");
  if (res.status === 403) throw new EngineUnavailable(403, "Your role does not include KaveonDB access.");
  if (!res.ok) throw new EngineUnavailable(res.status, `The server returned ${res.status}.`);
  return res.json() as Promise<T>;
}

export const fetchCluster = () => get<Cluster>("/cluster");
export const fetchQueries = () => get<QueryRecord[]>("/queries");
export const fetchQuery = (id: string) => get<QueryRecord>(`/queries/${encodeURIComponent(id)}`);

// ── Presentation helpers ──────────────────────────────────────────────────────

export function clientLabel(q: QueryRecord): string {
  const c = q.context || {};
  if (c.client === "kaveon-cli") return "Kaveon CLI";
  if (c.client === "kaveon-api") return c.source === "studio" ? "Studio" : "Kaveon API";
  return c.client || "HTTP";
}

export function userLabel(q: QueryRecord): string {
  const c = q.context || {};
  return c.user || c.principal || "Unknown";
}

export function bytes(v?: number | null): string {
  if (v == null) return "—";
  if (v < 1024) return `${v} B`;
  if (v < 1048576) return `${(v / 1024).toFixed(1)} KiB`;
  if (v < 1073741824) return `${(v / 1048576).toFixed(1)} MiB`;
  return `${(v / 1073741824).toFixed(2)} GiB`;
}

export function ms(v?: number | null): string {
  if (v == null) return "—";
  if (v < 1000) return `${v} ms`;
  if (v < 60000) return `${(v / 1000).toFixed(v < 10000 ? 2 : 1)} s`;
  return `${Math.floor(v / 60000)}m ${Math.round((v % 60000) / 1000)}s`;
}

export function us(v?: number | null): string {
  if (v == null) return "Not measured";
  if (v < 1000) return `${v} µs`;
  if (v < 1000000) return `${(v / 1000).toFixed(2)} ms`;
  return `${(v / 1000000).toFixed(2)} s`;
}

export function ns(v?: number | null): string {
  if (v == null) return "Not measured";
  if (v < 1000) return `${v} ns`;
  if (v < 1000000) return `${(v / 1000).toFixed(2)} µs`;
  if (v < 1000000000) return `${(v / 1000000).toFixed(2)} ms`;
  return `${(v / 1000000000).toFixed(2)} s`;
}

export function uptime(secs: number): string {
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m`;
  const h = Math.floor(secs / 3600), m = Math.floor((secs % 3600) / 60);
  if (h < 24) return `${h}h ${m}m`;
  return `${Math.floor(h / 24)}d ${h % 24}h`;
}

export function absoluteTime(msEpoch: number): string {
  return new Date(msEpoch).toLocaleString([], { month: "short", day: "numeric", hour: "numeric", minute: "2-digit", second: "2-digit" });
}

export function relativeTime(msEpoch: number, now = Date.now()): string {
  const d = Math.max(0, now - msEpoch);
  if (d < 5000) return "just now";
  if (d < 60000) return `${Math.floor(d / 1000)}s ago`;
  if (d < 3600000) return `${Math.floor(d / 60000)}m ago`;
  if (d < 86400000) return `${Math.floor(d / 3600000)}h ago`;
  return `${Math.floor(d / 86400000)}d ago`;
}

export function firstLine(sql: string, max = 160): string {
  const flat = sql.replace(/\s+/g, " ").trim();
  return flat.length > max ? flat.slice(0, max - 1) + "…" : flat;
}

export function errorExcerpt(error?: string | null, max = 96): string {
  if (!error) return "";
  const flat = error.replace(/\s+/g, " ").trim();
  return flat.length > max ? flat.slice(0, max - 1) + "…" : flat;
}

export function rate(v: number | undefined, unit: string): string {
  return Number.isFinite(v) ? `${(v as number).toLocaleString(undefined, { maximumFractionDigits: 1 })} ${unit}` : "Not measured";
}
