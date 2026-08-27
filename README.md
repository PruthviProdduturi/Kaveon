<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/reference/kaveon-logo-dark.svg?v=16">
  <img src="docs/reference/kaveon-logo.svg?v=16" alt="Kaveon — Talk to your data." width="380" />
</picture>

Connect your databases. Ask anything. Get instant answers with interactive charts — powered by a deterministic engine, not an LLM.

[![Deploy](https://github.com/PruthviProdduturi/Kaveon/actions/workflows/deploy.yml/badge.svg)](https://github.com/PruthviProdduturi/Kaveon/actions/workflows/deploy.yml) [![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE) [![Live Demo](https://img.shields.io/badge/Live-kaveon.vercel.app-4A9EE8?style=flat)](https://kaveon.vercel.app)

[**Try It**](https://kaveon.vercel.app) · [**Documentation**](https://kaveon.vercel.app/docs) · [**White Paper**](docs/whitepaper-nl-to-sql.md) · [**Architecture**](ARCHITECTURE.md)

</div>

---

## How It Works

You type a question. Kaveon figures out which dataset to query, generates SQL, executes it, picks the right chart type, and renders the answer inline — with an intelligent summary.

```
You: "Show confirmed cases by country"

Kaveon: Found 195 results. Top 3: United States (103.8M),
        India (45.0M), France (39.9M).
        [bar chart rendered inline]
```

No LLM. No API key. The **[DLM (Data Language Model)](docs/whitepaper-adaptive-context-routing.md)** — a per-dataset compiled context artifact that routes questions deterministically, answers the common cases from precomputed context with **no database scan at all**, and falls back to a single live query for the rest. Dashboard charts, filter dropdowns, and NL questions all serve from the same instant context layer.

---

## What You Get

| | |
|---|---|
| **DLM — Data Language Model** | Per-dataset compiled context artifact. Answers NL questions, powers dashboard charts, and populates filter dropdowns — all from precomputed context with no DB scan. [White paper](docs/whitepaper-adaptive-context-routing.md). |
| **Conversational querying** | Type questions in plain English. The DLM routes, resolves, and renders charts inline with intelligent summaries. |
| **37 chart types** | Bar, line, area, pie, scatter, heatmap, funnel, gauge, treemap, waterfall, calendar, 3D globe, and more. All interactive, all dark-mode aware. |
| **Dashboard builder** | Drag-and-drop canvas with resizable tiles, cross-chart filtering, shared filter bar, auto-refresh, and publishing. |
| **SQL Lab** | Monaco editor (VS Code-grade) with autocomplete, multi-tab sessions, query history, result caching, and inline AI. |
| **Semantic datasets** | Define dimensions, metrics, and joins once. Reuse across unlimited charts. Kaveon generates the SQL. |
| **Multi-source** | Microsoft Fabric SQL, Azure SQL, PostgreSQL, MySQL, StarRocks. Connect them all, query across them. |
| **Self-hosted** | Your infrastructure, your data, your rules. MIT licensed. |

---

## Architecture

```
Browser → Vercel (kaveon-web)
              ↓ /api/kaveon/* proxy
         Azure Container Apps (kaveon-api)
              ↓ SQL
         Your databases (Fabric, Azure SQL, PostgreSQL, MySQL)
```

Two services. One monorepo.

| Service | Stack | Deploy |
|---------|-------|--------|
| **kaveon-web** | Next.js 15, React 19, TypeScript, ECharts | Vercel |
| **kaveon-api** | FastAPI, Python 3.11, psycopg2, pyodbc | Azure Container Apps |

Auth is NextAuth (Auth.js v5) — GitHub, Google, and Microsoft Entra ID. Role-based access: Viewer → Analyst → Editor → Admin.

Infrastructure as code: [Bicep templates](infra/bicep/) for Azure resources.

---

## Quick Start

```bash
git clone https://github.com/PruthviProdduturi/Kaveon.git
cd Kaveon

# Frontend
pnpm install
pnpm --filter kaveon-web dev          # localhost:3000

# Backend
cd apps/kaveon-api
pip install -r requirements.txt
python main.py                         # localhost:8080
```

Configure `.env` at the repo root with your database connection. See [Setup Guide](docs/guides/deploy-vercel-azure-postgres.md) for full deployment.

---

## Documentation

| Guide | Description |
|-------|-------------|
| [Architecture](ARCHITECTURE.md) | System design, data flow, auth model |
| [DLM White Paper](docs/whitepaper-dlm.md) | The Data Language Model — compilation, intent resolution, freshness |
| [DLM Curation White Paper](docs/whitepaper-dlm-curation.md) | How Kaveon precomputes 375 answers across 10M rows |
| [Adaptive Context Routing](docs/whitepaper-adaptive-context-routing.md) | Per-element staleness scoring and query routing |
| [NL→SQL White Paper](docs/whitepaper-nl-to-sql.md) | Template-based deterministic translation (fallback layer) |
| [Patent Claims](docs/patent-adaptive-context-routing.md) | 21-claim filing-ready patent draft |
| [Charts](docs/guides/charts.md) | 37 chart types, dark mode, ECharts config |
| [Dashboards](docs/guides/dashboards.md) | Builder, filters, cross-filtering, publishing |
| [SQL Lab](docs/guides/sql-lab.md) | Monaco editor, query execution, caching |
| [Data Sources](docs/guides/data-sources.md) | Connecting databases |
| [Deployment](docs/guides/deploy-vercel-azure-postgres.md) | Vercel + Azure Container Apps + Azure PostgreSQL setup |

---

## Project Structure

```
Kaveon/
├── apps/
│   ├── kaveon-web/          Next.js frontend
│   └── kaveon-api/          FastAPI backend
├── packages/
│   └── types/               Shared TypeScript types
├── infra/
│   └── bicep/               Azure IaC templates
├── docs/                    Guides + white paper
├── scripts/                 Migration + utilities
└── demo/                    Data loading scripts
```

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

---

## License

[MIT](LICENSE) — built by [Pruthvi Prodduturi](https://github.com/PruthviProdduturi).
