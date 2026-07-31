# CLAUDE.md — LoomX

## Who

Pruthvi Prodduturi — Engineering Lead & Platform Architect. Types fast with typos; parse intent, don't flag them. Prefers terse responses — no trailing summaries. Never add Co-Authored-By to git commits.

## What is LoomX

LoomX is a **self-hosted enterprise analytics platform** (Apache Superset competitor) targeting Microsoft Fabric SQL, with Azure AD auth. Think private data command centre — query live data, build charts, assemble dashboards, AI-assisted analysis.

## Tech Stack

- **Frontend**: Next.js 15 / React 19, App Router, TypeScript, MSAL for Azure AD, ECharts 5.5 + echarts-gl 2.0, react-grid-layout, Monaco editor
- **Backend**: FastAPI (Python 3.11), pyodbc (MSSQL/Fabric), psycopg2 (PG), pymysql (MySQL), JWT RS256 via PyJWT + JWKS
- **Monorepo**: pnpm + Turborepo
- **Auth**: Azure AD (MSAL), with multi-provider planned. Azure Managed Identity token auth for Fabric SQL via `SQL_COPT_SS_ACCESS_TOKEN`
- **Databases supported**: Fabric SQL, Azure SQL, PostgreSQL, MySQL, Trino (upcoming), StarRocks (upcoming)

## Repo Layout

```
LoomX/
├── apps/
│   ├── loomx-api/      # FastAPI backend
│   │   ├── routers/    # API routes
│   │   ├── services/   # Business logic
│   │   ├── middleware/  # Auth, permissions
│   │   ├── models/     # Pydantic models
│   │   └── migrations/ # SQL migration files
│   └── loomx-web/      # Next.js frontend
│       ├── app/        # App Router pages
│       ├── components/ # React components
│       ├── hooks/      # Custom hooks (useAuth, useRole, etc.)
│       └── auth/       # MSAL auth context
└── packages/           # Shared packages
```

## DB Schema

Key tables: `datasets`, `dataset_dimensions`, `dataset_columns`, `dataset_metrics`, `charts`, `dashboards`, `saved_queries`, `query_history`, `favorites`, `data_sources`, `user_themes`, `activity`, `user_roles`

## Implemented Features

- Semantic datasets (star schema, dimensions, metrics, role-playing dims via COALESCE)
- 20+ chart types including 3D globe
- Dashboard builder: flat layout, drag-drop, rows/columns/tabs/text/headers/dividers
- Cross-filtering (click chart → filters other charts)
- SQL Lab with Monaco editor, multi-tab, history, saved queries
- Async query execution (POST /api/v1/sql/execute-async + GET /api/v1/sql/async/{job_id})
- Query result caching (SHA-256 keyed, TTL-based, 500 entry cap)
- CTAS endpoint (POST /api/v1/lab/ctas)
- Per-user favorites, themes, workspace activity
- Setup wizard for first-run metadata DB config
- Rate limiting: 120 SQL calls/user/min; Redis-backed if REDIS_URL set
- Multi-DB: Fabric SQL, Azure SQL, PostgreSQL, MySQL
- Connection pool warmup + 5-min heartbeat

## RBAC Design (Agreed Architecture)

4 roles synced to Azure AD App Registration:

| Role | Access |
|------|--------|
| Viewer | Default (no role in JWT). View published content only. No SQL Lab. |
| Analyst | Create/edit/delete own content. SQL Lab. See internal content. |
| Editor | Create/publish/edit anyone's content. Can set visibility=published. |
| Admin | Everything + manage data sources + assign roles + view all activity. |

Content visibility: `private` (owner + Admin), `internal` (Analyst/Editor/Admin, default), `published` (everyone).

Role resolution: JWT `roles` claim → `user_roles` DB table → default Viewer. First login when table empty → auto-bootstrap Admin.

Key implementation note: `require_auth` returns `UserContext` object (`.email`, `.role`), not just email string.

## AI Feature (Deferred)

NL→SQL, explain, optimize using `claude-sonnet-4-6` with live schema context. Config screen for API key (SetupModal pattern). Build when user returns to it.

## Upcoming Data Sources

Trino (ANSI SQL, schema.table) and StarRocks (MySQL-compatible, columnar OLAP, materialized views). Avoid MSSQL-specific syntax assumptions.

## Known Bugs

- Charts schema uses INT PK but some code paths treat id as NVARCHAR(36)
- `database_name="IDEASServingStoreLH"` in datasets.py:213 is a legitimate default — not a bug
- Async job store not cleaned on restart
- Connection strings stored plaintext in data_sources (no vault integration yet)
- In-process rate limiting doesn't share state across replicas (Redis fixes this)

## Critical Gaps vs Superset

1. Multi-auth (Azure AD only; need OAuth/SAML/LDAP)
2. Alerts & scheduled reports
3. Dashboard embedding (guest tokens)
4. Import/export dashboards/charts/datasets
5. Jinja SQL templating
6. Dataset certification / governance
7. Advanced filters (multi-select, range, cascading)
8. More chart types (Sankey, Treemap, Waterfall, Mixed time-series)
9. Non-Azure connectors (Snowflake, BigQuery, Databricks, Redshift)

## Preferences

- No Co-Authored-By in commits — ever.
- Terse responses, no summaries.
- Parse typos by intent.
