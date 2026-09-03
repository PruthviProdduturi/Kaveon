# HTTP API Reference

This is a route map, not a standalone API compatibility promise. FastAPI mounts
application routers in `api/main.py`; when the API is running, `/docs` is the
authoritative generated schema for request and response bodies.

## Studio and FastAPI path — current

Studio normally calls `/api/kaveon/*`, a same-origin Next.js proxy. The proxy reads
the server-side session and forwards identity headers plus `X-Proxy-Secret`.
FastAPI accepts that identity only when the secret matches `KAVEON_PROXY_SECRET`.
Provider bearer tokens and the explicitly configured local-development identity are
the other authentication modes. Health and initial-setup routes have different
requirements.

All paths below are relative to the FastAPI origin.

| Area | Prefix or routes | Purpose |
|---|---|---|
| Service | `/`, `/api/health`, `/docs` | Service metadata, health, generated OpenAPI UI |
| Authentication | `/api/connect`, `/api/disconnect`, `/api/auth/provider` | Connection and provider configuration |
| Setup/admin | `/api/v1/setup/*`, `/api/v1/admin/*` | First-run database setup and administration |
| Datasets | `/api/v1/datasets` | Dataset CRUD, columns, favorites, DLM generation/context/freshness |
| Charts | `/api/v1/charts` | Chart CRUD, summaries, favorites |
| Dashboards | `/api/v1/dashboards` | Dashboard CRUD, summaries, favorites, DLM curation |
| Data sources | `/api/v1/data-sources` | Registration, metadata, favorites; connection test is currently a stub |
| SQL | `/api/v1/sql/*` | SQL generation, execution, detached jobs, result cache, filter values |
| SQL Lab | `/api/v1/lab/*` | Discovery, saved queries, execution, CTAS, history, distinct values |
| DLM | `/api/v1/dlm/*` | Routing, ask, chart serving, filter values, coverage, cache and freshness operations |
| Context router | `/api/v1/context/*` | Build, validity, and adaptive-context ask endpoints |
| Chat | `/api/v1/chat`, `/api/v1/chat/history*` | Assistant and conversation persistence |
| User state | `/api/v1/favorites`, `/api/v1/theme`, `/api/v1/user/recents`, `/api/v1/users/me` | Per-user state |
| Optional AI | `/api/v1/ai/*` | Provider configuration, personal keys, and hosted-model chat |

### Important behavior

- Each platform query targets one selected SQL source. Cross-source federation is
  not implemented.
- `POST /api/v1/data-sources/{id}/test` currently returns a not-implemented message;
  the setup probe endpoints perform real connectivity checks.
- Detached SQL jobs and cached results live in process memory. Restarting the API
  loses that state, and multiple replicas need external coordination that is not
  currently implemented.
- Authorization differs by route. Do not infer write permission merely from an
  authenticated session; inspect the generated OpenAPI schema and router dependency.

## Engine HTTP path — alpha

The Rust server exposes these routes:

| Method | Path | Current behavior |
|---|---|---|
| `POST` | `/v1/statement` | Synchronously parse, plan, execute, materialize, and retain a query result |
| `GET` | `/v1/query` | Return up to 100 newest process-local query records |
| `GET` | `/v1/query/{query_id}` | Return retained lifecycle, context, structured logical plan, result, and scan telemetry |
| `DELETE` | `/v1/query/{query_id}` | Delete a retained record; does not cancel running computation |
| `GET` | `/v1/cluster` | Coordinator and discovered-worker state |
| `GET` | `/v1/node` | Current node information |
| `POST` | `/v1/node/heartbeat` | Register a worker heartbeat on a coordinator |
| `GET` | `/v1/catalog` | List catalogs |
| `GET` | `/v1/catalog/{catalog}/schema` | List schemas |
| `GET` | `/v1/catalog/{catalog}/schema/{schema}/table` | List tables |
| `GET` | `/health`, `/ready`, `/ui` | Liveness, catalog readiness, and operational UI |

The Engine server has permissive CORS and no authentication, authorization, TLS,
quota, cancellation, or distributed fragment execution. Keep it behind a trusted
boundary during alpha.

The statement JSON body requires `query`. Clients may also provide `source`,
`client`, `time_zone`, `client_tags`, and `result_delivery`. These identify the
submitting application and session; they are not trusted user identity. Principal
and client address remain unavailable until authenticated request plumbing is
implemented.
