/**
 * The product page's benchmark figures — the files
 * scripts/benchmark-chart-data.py writes to public/benchmarks/. Read on the
 * server at build time so the page carries the numbers with no client fetch,
 * no CORS and no middleware exposure; a missing or malformed file yields null
 * and that figure is simply not offered.
 */
import { promises as fs } from "fs";
import path from "path";
import type { BenchmarkFigures, Cluster, LatencyFigure, ThroughputFigure } from "./benchmarkTypes";

export type * from "./benchmarkTypes";

export const BENCHMARK_FILES = {
  clickbench: "benchmarks/clickbench-2026-09.json",
  tpch: "benchmarks/tpch-2026-09.json",
  throughput: "benchmarks/throughput-2026-09.json",
} as const;

function hasCluster(v: { cluster?: Partial<Cluster> }): boolean {
  return !!v.cluster && typeof v.cluster.workers === "number" && typeof v.cluster.node_sku === "string";
}

function isLatency(value: unknown): value is LatencyFigure {
  if (!value || typeof value !== "object") return false;
  const v = value as Partial<LatencyFigure>;
  return v.kind === "latency" && Array.isArray(v.statements) && v.statements.length > 0
    && typeof v.rounds === "number" && v.rounds > 0 && typeof v.date === "string" && typeof v.title === "string"
    && !!v.summary && typeof v.summary.statements === "number" && hasCluster(v);
}

function isThroughput(value: unknown): value is ThroughputFigure {
  if (!value || typeof value !== "object") return false;
  const v = value as Partial<ThroughputFigure>;
  return v.kind === "throughput" && Array.isArray(v.groups) && v.groups.length > 0
    && v.groups.every((g) => typeof g.clients === "number" && Array.isArray(g.rounds) && g.rounds.length > 0)
    && typeof v.duration_seconds === "number" && typeof v.date === "string" && hasCluster(v);
}

async function readPublicJson(file: string): Promise<unknown> {
  const raw = await fs.readFile(path.join(process.cwd(), "public", file), "utf-8");
  return JSON.parse(raw) as unknown;
}

async function load<T>(file: string, guard: (value: unknown) => value is T): Promise<T | null> {
  try {
    const parsed = await readPublicJson(file);
    return guard(parsed) ? parsed : null;
  } catch {
    return null;
  }
}

export async function loadBenchmarkFigures(): Promise<BenchmarkFigures> {
  const [clickbench, tpch, throughput] = await Promise.all([
    load(BENCHMARK_FILES.clickbench, isLatency),
    load(BENCHMARK_FILES.tpch, isLatency),
    load(BENCHMARK_FILES.throughput, isThroughput),
  ]);
  return { clickbench, tpch, throughput };
}

