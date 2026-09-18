/**
 * The product page's ClickBench figure — the file scripts/benchmark-chart-data.py
 * writes to public/benchmarks/. Read on the server at build time so the page
 * carries the numbers with no client fetch, no CORS and no middleware
 * exposure; a missing or malformed file yields null and the section renders
 * nothing.
 */
import { promises as fs } from "fs";
import path from "path";

export interface ClickBenchStatement {
  id: string;
  label: string;
  sql: string;
  /** Median over rounds of each round's median of its timed executions; null when a round failed. */
  seconds: number | null;
  round_medians: (number | null)[];
}

export interface ClickBenchFigure {
  suite: string;
  engine: string;
  /** ISO date of the campaign directory. */
  date: string;
  rounds: number;
  executions_per_round: number;
  rows: number | null;
  table: string;
  cluster: { workers: number; node_sku: string; memory_per_query_per_worker: string };
  engine_digests: string[];
  /** Repo-relative path of the round records. */
  record_path: string;
  summary: {
    statements: number;
    ran_every_round: number;
    under_1s: number;
    under_10s: number;
    slowest: { id: string; seconds: number } | null;
  };
  statements: ClickBenchStatement[];
}

export const CLICKBENCH_FILE = "benchmarks/clickbench-2026-09.json";

function isFigure(value: unknown): value is ClickBenchFigure {
  if (!value || typeof value !== "object") return false;
  const v = value as Partial<ClickBenchFigure>;
  return Array.isArray(v.statements) && v.statements.length > 0
    && typeof v.rounds === "number" && typeof v.date === "string"
    && !!v.summary && typeof v.summary.statements === "number"
    && !!v.cluster && typeof v.cluster.workers === "number";
}

export async function loadClickBenchFigure(): Promise<ClickBenchFigure | null> {
  try {
    const raw = await fs.readFile(path.join(process.cwd(), "public", CLICKBENCH_FILE), "utf-8");
    const parsed: unknown = JSON.parse(raw);
    return isFigure(parsed) ? parsed : null;
  } catch {
    return null;
  }
}
