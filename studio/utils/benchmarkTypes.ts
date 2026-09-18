/**
 * The shapes of the product page's benchmark figures — what
 * scripts/benchmark-chart-data.py writes. No Node imports: this module is
 * shared with client components; the server-side loader is utils/benchmarks.ts.
 */
export interface LatencyStatement {
  id: string;
  label: string;
  sql: string;
  /** Median over rounds of each round's median of its timed executions; null when a round failed. */
  seconds: number | null;
  round_medians: (number | null)[];
}

export interface Cluster {
  workers: number;
  node_sku: string;
  memory_per_query_per_worker: string;
}

export interface LatencyFigure {
  kind: "latency";
  suite: string;
  /** Suite name with its scale where one applies, e.g. "TPC-H SF100". */
  title: string;
  engine: string;
  /** ISO date of the campaign directory. */
  date: string;
  rounds: number;
  executions_per_round: number;
  rows: number | null;
  table: string | null;
  scale_factor: number | null;
  cluster: Cluster;
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
  statements: LatencyStatement[];
}

export interface ThroughputRound {
  round: number;
  /** Successful exact executions divided by the measured span. */
  executions_per_second: number | null;
  /** The span the rate is divided by: the window plus the time the statements in flight at its end took to return. */
  elapsed_seconds: number | null;
  successful: number;
  failures: number;
  rejections: number;
  ties: number;
}

export interface ThroughputGroup {
  clients: number;
  rounds: ThroughputRound[];
  median_executions_per_second: number | null;
}

export interface ThroughputFigure {
  kind: "throughput";
  suite: string;
  title: string;
  engine: string;
  date: string;
  duration_seconds: number;
  warmup_seconds: number;
  cluster: Cluster;
  engine_digests: string[];
  record_path: string;
  groups: ThroughputGroup[];
}

export interface BenchmarkFigures {
  clickbench: LatencyFigure | null;
  tpch: LatencyFigure | null;
  throughput: ThroughputFigure | null;
}

export function hasAnyFigure(figures: BenchmarkFigures | null | undefined): figures is BenchmarkFigures {
  return !!figures && (figures.clickbench !== null || figures.tpch !== null || figures.throughput !== null);
}
