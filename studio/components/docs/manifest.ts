export type DocsItem = {
  title: string;
  href: `/docs${string}`;
  description: string;
  keywords?: readonly string[];
};

export type DocsGroup = {
  label: string;
  items: readonly DocsItem[];
};

export const DOCS_NAV: readonly DocsGroup[] = [
  {
    label: "Getting Started",
    items: [
      { title: "Introduction", href: "/docs", description: "What Kaveon is and where to begin.", keywords: ["overview"] },
      { title: "Quickstart", href: "/docs/quickstart", description: "Connect data and build a first chart.", keywords: ["install", "start"] },
      { title: "Core concepts", href: "/docs/concepts", description: "Sources, datasets, charts, dashboards, and questions." },
    ],
  },
  {
    label: "Studio",
    items: [
      { title: "SQL Lab", href: "/docs/sql-lab", description: "Explore data with the query editor." },
      { title: "AI · NL→SQL", href: "/docs/nl-to-sql", description: "Turn questions into deterministic queries.", keywords: ["natural language", "ai"] },
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
      { title: "Freshness Algorithm", href: "/docs/freshness", description: "Route between context and live data." },
    ],
  },
  {
    label: "Build & Operate",
    items: [
      { title: "Architecture", href: "/docs/architecture", description: "Current platform structure and data flow." },
      { title: "Kaveon Engine", href: "/docs/engine", description: "Run the alpha Rust query engine.", keywords: ["rust", "parquet", "cli"] },
      { title: "API Reference", href: "/docs/api-reference", description: "Find platform and Engine API surfaces.", keywords: ["rest", "endpoint"] },
      { title: "Auth & RBAC", href: "/docs/auth", description: "Configure identity, roles, and visibility.", keywords: ["security"] },
      { title: "Deployment", href: "/docs/deployment", description: "Deploy Studio, API, and the data layer.", keywords: ["vercel", "azure"] },
      { title: "Operations", href: "/docs/operations", description: "Operate, verify, and troubleshoot Kaveon.", keywords: ["health", "ready", "backup"] },
    ],
  },
  {
    label: "Research",
    items: [
      { title: "Papers & Patents", href: "/docs/research", description: "Read the technical papers and patent disclosure.", keywords: ["whitepaper", "adaptive routing", "curation"] },
    ],
  },
] as const;

export const DOCS_ITEMS = DOCS_NAV.flatMap((group) => group.items);
