<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/reference/kaveon-logo-dark.svg?v=16">
  <img src="docs/reference/kaveon-logo.svg?v=16" alt="Kaveon — Talk to your data." width="380" />
</picture>

<br/>

Connect your databases. Ask anything. Get instant answers with interactive charts<br/>
powered by the **DLM (Data Language Model)** — not an LLM.

<br/>

[![Deploy](https://github.com/PruthviProdduturi/Kaveon/actions/workflows/deploy.yml/badge.svg)](https://github.com/PruthviProdduturi/Kaveon/actions/workflows/deploy.yml) [![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE) [![Live Demo](https://img.shields.io/badge/demo-kaveon.vercel.app-4A9EE8)](https://kaveon.vercel.app) [![504M Rows](https://img.shields.io/badge/scale-504M_rows-38a169)](https://kaveon.vercel.app) [![Patent](https://img.shields.io/badge/patent-in_process-d69e2e)](docs/patent-adaptive-context-routing.md)

[![Python](https://img.shields.io/badge/Python-3.11-3776AB?logo=python&logoColor=fff)](apps/kaveon-api/) [![TypeScript](https://img.shields.io/badge/TypeScript-5-3178C6?logo=typescript&logoColor=fff)](apps/kaveon-web/) [![Next.js](https://img.shields.io/badge/Next.js-15-000?logo=nextdotjs&logoColor=fff)](apps/kaveon-web/) [![FastAPI](https://img.shields.io/badge/FastAPI-009688?logo=fastapi&logoColor=fff)](apps/kaveon-api/) [![PostgreSQL](https://img.shields.io/badge/PostgreSQL-18-4169E1?logo=postgresql&logoColor=fff)](infra/bicep/)

[**Try It**](https://kaveon.vercel.app) · [**Documentation**](https://kaveon.vercel.app/docs) · [**White Paper**](docs/whitepaper-dlm.md) · [**Architecture**](ARCHITECTURE.md) · [**Patent**](docs/patent-adaptive-context-routing.md)

</div>

---

## How It Works

You type a question. Kaveon figures out which dataset to query, generates SQL, executes it, picks the right chart type, and renders the answer inline — with an intelligent summary.

```
You: "Show confirmed cases by country"

Kaveon: Found 195 results. Top 3: United States (103.8M),
        India (45.0M), France (39.9M).
        ⚡ From context · no DB scan
        [bar chart rendered inline]
```

No LLM. No API key. The **[DLM (Data Language Model)](docs/whitepaper-adaptive-context-routing.md)** is a per-dataset compiled context artifact that routes questions deterministically, answers common cases from precomputed context with **no database scan at all**, and falls back to a single live query for the rest.

---

## DLM — Data Language Model

> [White Paper](docs/whitepaper-dlm.md) · [Curation at Scale](docs/whitepaper-dlm-curation.md) · [Adaptive Context Routing](docs/whitepaper-adaptive-context-routing.md) · [Patent Claims](docs/patent-adaptive-context-routing.md)

<div align="center">
  <img src="docs/reference/kaveon-dlm-flow.svg" alt="DLM architecture: compile once, answer instantly" width="820" />
</div>

<br/>

| | |
|:---:|---|
| **Compile once** | Profile `pg_stats`, precompute 375 answers (totals + per-dimension breakdowns + 2-dim combos), build a value index mapping terms to columns and values |
| **Answer instantly** | NL questions, dashboard charts, and filter dropdowns all served from an in-memory dict — **~5ms, zero warehouse load** |
| **Fall back cleanly** | Complex shapes (time-series, multi-filter combos) assemble a single live query — one scan, cached |
| **Stay fresh** | Per-element staleness scoring from DBMS counters (`n_mod_since_analyze`) — no re-query to check freshness |

**Measured:** Over a 10.1M-row usage dataset, "current usage" drops from **~15s live → ~1.5s from context**. Dashboard charts render from context in **~5ms** end-to-end.

---

## Architecture

<div align="center">
  <img src="docs/reference/kaveon-architecture.svg" alt="Kaveon architecture: browser to Next.js to FastAPI to your databases" width="820" />
</div>

<br/>

Two services. One monorepo. Zero exposed tokens.

| Service | Stack | Deploy |
|---------|-------|--------|
| **kaveon-web** | Next.js 15, React 19, TypeScript, ECharts, Monaco | Vercel |
| **kaveon-api** | FastAPI, Python 3.11, psycopg2, pyodbc | Azure Container Apps |

The browser never holds an API token — all calls go same-origin to a Next.js proxy that stamps identity server-side via NextAuth (Auth.js v5).

---

## Features

| | |
|---|---|
| **🧠 DLM — Data Language Model** | Per-dataset compiled context artifact. Answers NL questions, powers dashboard charts, and populates filter dropdowns — all from precomputed context with no DB scan. [White paper →](docs/whitepaper-dlm.md) |
| **💬 Conversational querying** | Type questions in plain English. The DLM routes, resolves, and renders charts inline with intelligent summaries. |
| **📊 37 chart types** | Bar, line, area, pie, scatter, heatmap, funnel, gauge, treemap, waterfall, calendar, 3D globe, and more. All interactive, all dark-mode aware. |
| **📋 Dashboard builder** | Drag-and-drop canvas with resizable tiles, cross-chart filtering, shared filter bar, auto-refresh, and publishing. |
| **⌨️ SQL Lab** | Monaco editor (VS Code-grade) with autocomplete, multi-tab sessions, query history, result caching, and inline AI. |
| **🔗 Semantic datasets** | Define dimensions, metrics, and joins once. Reuse across unlimited charts. Kaveon generates the SQL. |
| **🗄️ Multi-source** | Microsoft Fabric SQL, Azure SQL, PostgreSQL, MySQL, StarRocks, Trino. Connect them all, query across them. |
| **🔒 Self-hosted** | Your infrastructure, your data, your rules. MIT licensed. |

---

## Tech Stack

<table>
<tr>
<td align="center" width="100"><strong>Frontend</strong></td>
<td>Next.js 15 · React 19 · TypeScript · ECharts + GL · Monaco Editor · NextAuth (Auth.js v5)</td>
</tr>
<tr>
<td align="center" width="100"><strong>Backend</strong></td>
<td>FastAPI · Python 3.11 · psycopg2 · pyodbc · pymysql · Gunicorn + Uvicorn</td>
</tr>
<tr>
<td align="center" width="100"><strong>Database</strong></td>
<td>Azure PostgreSQL 18 (kaveonmeta + kaveon) · Managed Identity auth · Connection pooling</td>
</tr>
<tr>
<td align="center" width="100"><strong>Infra</strong></td>
<td>Azure Container Apps · Vercel · Bicep IaC · GitHub Actions CI/CD</td>
</tr>
<tr>
<td align="center" width="100"><strong>Auth</strong></td>
<td>OAuth (GitHub · Google · Microsoft Entra ID) · RBAC (Viewer → Analyst → Editor → Admin)</td>
</tr>
<tr>
<td align="center" width="100"><strong>Scale</strong></td>
<td>504M rows · 10 datasets · 9 dashboards · 375 precomputed answers per dataset · HLL sketch cuboids</td>
</tr>
</table>

---

## Documentation

| Guide | Description |
|-------|-------------|
| [Architecture](ARCHITECTURE.md) | System design, data flow, auth model |
| [DLM White Paper](docs/whitepaper-dlm.md) | The Data Language Model — compilation, intent resolution, freshness |
| [DLM Curation](docs/whitepaper-dlm-curation.md) | How Kaveon precomputes 375 answers across 10M rows |
| [Adaptive Context Routing](docs/whitepaper-adaptive-context-routing.md) | Per-element staleness scoring and query routing |
| [NL→SQL White Paper](docs/whitepaper-nl-to-sql.md) | Template-based deterministic translation (fallback layer) |
| [Patent Claims](docs/patent-adaptive-context-routing.md) | 21-claim filing-ready patent draft |
| [Charts](docs/guides/charts.md) | 37 chart types, dark mode, ECharts config |
| [Dashboards](docs/guides/dashboards.md) | Builder, filters, cross-filtering, publishing |
| [SQL Lab](docs/guides/sql-lab.md) | Monaco editor, query execution, caching |
| [Data Sources](docs/guides/data-sources.md) | Connecting databases |
| [Deployment](docs/guides/deploy-vercel-azure-postgres.md) | Vercel + Azure Container Apps + Azure PostgreSQL setup |

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

Configure `.env` at the repo root with your database connection. See [Deployment Guide](docs/guides/deploy-vercel-azure-postgres.md) for full setup.

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
├── docs/                    Guides + white papers + patent
├── scripts/                 Migration + utilities
└── demo/                    Data loading scripts
```

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

---

## License

[MIT](LICENSE) — built by [Pruthvi Prodduturi](https://github.com/PruthviProdduturi).
