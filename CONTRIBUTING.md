# Contributing to Kaveon

Thanks for your interest in contributing to Kaveon — the Analyze module of the Kaveon suite. This guide will get you set up.

## Prerequisites

- Node.js 22+ (the repo pins Node 20 in `.nvmrc`, but pnpm 10 needs Node ≥ 22.13 for `node:sqlite`)
- pnpm 10 (`corepack prepare pnpm@10 --activate`)
- Python 3.11+
- ODBC Driver 18 for SQL Server (Fabric SQL / Azure SQL) — see the README Prerequisites section

## Local Development

### Web (Next.js)

```bash
pnpm install

# Run locally (override to :3002 when running the whole Kaveon suite)
pnpm --filter kaveon-studio dev

# Lint & type check
pnpm --filter kaveon-studio lint
pnpm --filter kaveon-studio type-check

# Build
pnpm --filter kaveon-studio build
```

### API (FastAPI)

```bash
cd api
python -m venv venv
venv\Scripts\activate        # Windows
# source venv/bin/activate   # macOS / Linux
pip install -r requirements.txt

# Run locally (:8080)
python main.py

# Run tests
pytest -q
```

Both services read the single `.env` at the repo root. Copy `.env.example` to `.env`; local login works with no config (`admin` / `admin` on first run).

## Code Standards

### TypeScript
- **Linter**: ESLint (`eslint-config-next`)
- **Type checker**: `tsc --noEmit` (strict mode)
- **Framework**: Next.js 15 App Router, React 19
- State-changing fetches use `studio/utils/msalFetch.ts`, never bare `fetch`; Studio still uses `@azure/msal-browser` for Microsoft identity integration.

### Python
- **Type**: FastAPI + Pydantic v2, Python 3.11
- **Tests**: pytest + pytest-mock
- **SQL safety is non-negotiable** — see the Security Checklist in [SECURITY.md](SECURITY.md) before touching any router or service:
  - Identity comes from the `require_auth` dependency, never a client header.
  - All dynamic identifiers pass through `quote_identifier()`.
  - All values are bound as pyodbc parameters, never string-interpolated.
  - Create/update/delete endpoints use `require_min_role(...)`, not just `require_auth`.

## Commit Messages

Use conventional prefixes:

```
feat: add sankey chart type
fix: resolve Fabric serverless cold-start timeout
chore: bump echarts to 5.6
docs: expand data-source setup guide
refactor: simplify query_generator JOIN logic
test: add dataset visibility tests
security: tighten CORS allowlist
perf: parallelise dashboard chart preload
```

Body should explain **why**, not what. The diff shows what. **Never add `Co-Authored-By` trailers.**

## Pull Requests

1. Branch off `dev`: `feat/my-feature` or `fix/my-fix`
2. Make your changes with tests where practical
3. Ensure CI passes: web lint / type-check / build, API syntax / tests, secret scan
4. Open a PR against `dev` using the PR template
5. One approval required

## What to Contribute

Check [open issues](https://github.com/PruthviProdduturi/Kaveon/issues) for things to work on. Good first issues are labeled `good first issue`.

### Areas that need help

- Additional chart types (Sankey, mixed time-series, network graph)
- Non-Azure connectors (Snowflake, BigQuery, Databricks, Redshift)
- Dashboard import/export
- Alerts & scheduled reports
- Dashboard embedding (guest tokens)
