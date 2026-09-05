export type DocsItem = {
  title: string;
  href: `/docs${string}`;
  description: string;
  keywords?: readonly string[];
  status?: "Stable" | "Beta" | "Alpha" | "Reference";
  lastVerified?: string;
};

export type DocsGroup = {
  label: string;
  /** Rendered always-open and non-collapsible in the sidebar. */
  pinned?: boolean;
  items: readonly DocsItem[];
};

export const DOCS_NAV: readonly DocsGroup[] = [
  {
    label: "Getting Started",
    pinned: true,
    items: [
      { title: "Introduction", href: "/docs", description: "What Kaveon is and where to begin.", keywords: ["overview"] },
      { title: "Quickstart", href: "/docs/quickstart", description: "Connect data and build a first chart.", keywords: ["install", "start"] },
      { title: "Core concepts", href: "/docs/concepts", description: "Sources, datasets, charts, dashboards, and questions." },
    ],
  },
  {
    label: "Platform",
    items: [
      { title: "Architecture", href: "/docs/architecture", description: "Current platform structure and data flow." },
      { title: "API Reference", href: "/docs/api-reference", description: "Find platform and Engine API surfaces.", keywords: ["rest", "endpoint"] },
      { title: "Connector Matrix", href: "/docs/connectors", description: "Compare source registration, execution, and DLM support.", keywords: ["mysql", "postgresql", "fabric", "trino"] },
    ],
  },
  {
    label: "Studio",
    items: [
      { title: "SQL Lab", href: "/docs/sql-lab", description: "Explore data with the query editor." },
      { title: "Chart Builder", href: "/docs/charts", description: "Create reusable interactive visualizations." },
      { title: "Dashboards", href: "/docs/dashboards", description: "Compose and publish analytical canvases." },
      { title: "Semantic Datasets", href: "/docs/datasets", description: "Define reusable metrics and dimensions." },
      { title: "Data Sources", href: "/docs/data-sources", description: "Connect and manage databases.", keywords: ["connector"] },
    ],
  },
  {
    label: "Intelligence",
    items: [
      { title: "Data Language Model", href: "/docs/dlm", description: "Understand Kaveon's compiled semantic layer.", keywords: ["dlm"] },
      { title: "DLM · NL→SQL", href: "/docs/nl-to-sql", description: "Turn questions into deterministic queries.", keywords: ["natural language", "dlm"] },
      { title: "Freshness Algorithm", href: "/docs/freshness", description: "Route between context and live data." },
    ],
  },
  {
    label: "Engine",
    items: [
      { title: "Kaveon Engine", href: "/docs/engine", description: "Run the alpha Rust query engine.", keywords: ["rust", "parquet", "cli"] },
      { title: "Engine Architecture", href: "/docs/engine/architecture", description: "Understand processes, crates, startup, and boundaries.", keywords: ["coordinator", "worker", "startup"], status: "Alpha", lastVerified: "September 4, 2026" },
      { title: "Engine SQL", href: "/docs/engine/sql", description: "Track committed SQL semantics and integration gates.", keywords: ["sql", "function", "window"], status: "Alpha", lastVerified: "September 4, 2026" },
      { title: "Distributed Runtime", href: "/docs/engine/distributed", description: "Follow stages, fragments, tasks, exchange, retry, and cancellation.", keywords: ["stage", "fragment", "exchange"], status: "Alpha", lastVerified: "September 4, 2026" },
      { title: "Storage & Catalogs", href: "/docs/engine/storage", description: "Read Parquet/Delta and understand native metadata.", keywords: ["parquet", "delta", "catalog"], status: "Alpha", lastVerified: "September 4, 2026" },
      { title: "Engine Memory", href: "/docs/memory", description: "Understand admission, reservations, spill, and current safety boundaries.", keywords: ["memory", "spill", "admission"], status: "Alpha", lastVerified: "September 4, 2026" },
      { title: "SQL Compatibility", href: "/docs/sql-compatibility", description: "See what the alpha Engine can execute.", keywords: ["order by", "aggregate", "operator"] },
    ],
  },
  {
    label: "Deploy & Operate",
    items: [
      { title: "Deployment", href: "/docs/deployment", description: "Deploy Studio, API, and the data layer.", keywords: ["vercel", "azure"] },
      { title: "Auth & RBAC", href: "/docs/auth", description: "Configure identity, roles, and visibility.", keywords: ["security"] },
      { title: "Operations", href: "/docs/operations", description: "Operate, verify, and troubleshoot Kaveon.", keywords: ["health", "ready", "backup"] },
      { title: "Troubleshooting", href: "/docs/troubleshooting", description: "Diagnose authentication, database, and Engine issues.", keywords: ["cors", "proxy", "logs"] },
      { title: "Upgrades", href: "/docs/upgrades", description: "Plan upgrades under the current pre-1.0 policy.", keywords: ["version", "migration", "rollback"] },
      { title: "Releases", href: "/docs/releases", description: "Release channels and changelog requirements.", keywords: ["changelog", "engine-dev"] },
    ],
  },
  {
    label: "Research",
    items: [
      { title: "Papers & Patents", href: "/docs/research", description: "Read the technical papers and patent disclosure.", keywords: ["whitepaper", "adaptive routing", "curation"] },
      { title: "Kaveon vs Trino", href: "/docs/research/trino", description: "Compare architecture, execution maturity, and proof requirements.", keywords: ["trino", "distributed sql", "benchmark"], status: "Reference", lastVerified: "September 4, 2026" },
      { title: "Engine vs Fabric SQL", href: "/docs/research/fabric-sql-endpoint", description: "Compare Kaveon Engine with the Fabric Lakehouse SQL analytics endpoint.", keywords: ["fabric", "sql analytics endpoint", "onelake"], status: "Reference", lastVerified: "September 4, 2026" },
    ],
  },
] as const;

export const DOCS_ITEMS = DOCS_NAV.flatMap((group) => group.items);

export function docsMeta(href: string) {
  const group = DOCS_NAV.find((candidate) => candidate.items.some((item) => item.href === href));
  const item = group?.items.find((candidate) => candidate.href === href);
  if (!group || !item) return undefined;
  return {
    group: group.label,
    ...item,
    status: item.status ?? (href === "/docs/engine" ? "Alpha" : href === "/docs/research" ? "Reference" : "Stable"),
    lastVerified: item.lastVerified ?? "September 2, 2026",
  };
}
