"""
Kaveon API — FastAPI entry point.
Replaces the Node.js/Express API + Python Flask proxy with a single service.
"""

import threading
from contextlib import asynccontextmanager

from fastapi import FastAPI, Request
from fastapi.responses import JSONResponse, Response
from fastapi.middleware.cors import CORSMiddleware
import time

from config import settings
from middleware.errors import AppError, app_error_handler, generic_error_handler
from database.pool import LargeIntResponse, TokenAuthError
from database.warmup import start_warmup_and_heartbeat

# Routers
from routers import (
    ai,
    auth,
    auth_config,
    health,
    datasets,
    charts,
    dashboards,
    metadata_summary,
    data_sources,
    favorites,
    lab,
    local_auth,
    sql,
    theme,
    setup,
    users,
)


# ── Lifespan ──────────────────────────────────────────────────────────────────

@asynccontextmanager
async def lifespan(app: FastAPI):
    import os as _os

    # Determine whether a metadata DB is configured by reading the .env file
    # directly — os.environ / settings may hold stale values from a previous
    # process (e.g. uvicorn --reload), so the file is the source of truth.
    from routers.setup import _read_env_vars as _get_env
    _cfg = _get_env()
    _db_configured = bool(
        _cfg["database"] and (
            _cfg["endpoint"] or  # Fabric SQL / Azure SQL
            _cfg["host"]         # PostgreSQL / MySQL
        )
    )
    # Sync to os.environ so pool/other code that reads os.environ gets the correct value
    _os.environ["METADATA_DB_TYPE"]  = _cfg["db_type"]
    _os.environ["METADATA_ENDPOINT"] = _cfg["endpoint"]
    _os.environ["METADATA_DATABASE"] = _cfg["database"]
    _os.environ["METADATA_HOST"]     = _cfg["host"]
    _os.environ["METADATA_PORT"]     = _cfg["port"]

    # Startup: kick off pool warmup + heartbeat in the background
    if _db_configured:
        threading.Thread(target=start_warmup_and_heartbeat, daemon=True).start()
        print("[API] Connection pool warmup started.")
    else:
        print("[API] No metadata database configured — starting in setup mode.")
        print("[API] Setup wizard is available. Default login: admin / admin")

    # Bootstrap local admin user (creates admin row + ensures Admin role).
    # Skipped gracefully when no DB is configured — the hardcoded _BOOTSTRAP_HASH
    # in services/local_auth.py handles the pre-DB admin/admin fallback instead.
    if _db_configured:
        try:
            import services.local_auth as _local_auth_svc
            _local_auth_svc.bootstrap_admin_if_needed()
        except Exception as _e:
            print(f"[API] bootstrap_admin_if_needed skipped (will retry on next restart): {_e}")

    yield
    # Shutdown: pools close via GC; nothing explicit needed


# ── App ───────────────────────────────────────────────────────────────────────

app = FastAPI(
    title="Kaveon API",
    version="2.0.0",
    default_response_class=LargeIntResponse,
    lifespan=lifespan,
)

_CORS_HEADERS = {
    "Access-Control-Allow-Origin": "*",
    "Access-Control-Allow-Methods": "GET, POST, PUT, PATCH, DELETE, OPTIONS",
    "Access-Control-Allow-Headers": "Authorization, Content-Type, x-user-email, Cache-Control",
    "Access-Control-Max-Age": "600",
}

async def _token_auth_handler(request: Request, exc: TokenAuthError) -> JSONResponse:
    print(f"[API] Token auth failure — {request.method} {request.url.path}")
    return JSONResponse(
        status_code=401,
        content={"error": {"code": "token_auth_failed", "message": "Azure AD token authentication failed. Please sign in again."}},
    )


# Exception handlers
app.add_exception_handler(AppError, app_error_handler)
app.add_exception_handler(TokenAuthError, _token_auth_handler)
app.add_exception_handler(Exception, generic_error_handler)


# ── Security headers ──────────────────────────────────────────────────────────

@app.middleware("http")
async def security_headers(request: Request, call_next):
    if request.method == "OPTIONS":
        return Response(status_code=200, headers=_CORS_HEADERS)
    response = await call_next(request)
    response.headers.update(_CORS_HEADERS)
    response.headers["X-Content-Type-Options"] = "nosniff"
    response.headers["X-Frame-Options"] = "DENY"
    response.headers["Strict-Transport-Security"] = "max-age=31536000; includeSubDomains"
    response.headers["Referrer-Policy"] = "strict-origin-when-cross-origin"
    response.headers["X-Permitted-Cross-Domain-Policies"] = "none"
    response.headers["Content-Security-Policy"] = "default-src 'none'"
    return response


# ── Request logging ───────────────────────────────────────────────────────────

@app.middleware("http")
async def log_requests(request: Request, call_next):
    start = time.time()
    response = await call_next(request)
    duration_ms = int((time.time() - start) * 1000)
    print(f"[{request.method}] {request.url.path} - {response.status_code} ({duration_ms}ms)")
    return response


# ── Routers ───────────────────────────────────────────────────────────────────

app.include_router(auth.router,             prefix="/api")
app.include_router(auth_config.router,      prefix="/api")
app.include_router(local_auth.router,       prefix="/api")
app.include_router(health.router,           prefix="/api")
app.include_router(metadata_summary.router, prefix="/api/v1")
app.include_router(favorites.router,        prefix="/api/v1")
app.include_router(theme.router,            prefix="/api/v1")
app.include_router(lab.router,              prefix="/api/v1")
app.include_router(sql.router,              prefix="/api/v1")
app.include_router(datasets.router,         prefix="/api/v1")
app.include_router(charts.router,           prefix="/api/v1")
app.include_router(dashboards.router,       prefix="/api/v1")
app.include_router(data_sources.router,     prefix="/api/v1")
app.include_router(setup.router,            prefix="/api/v1")
app.include_router(users.router,            prefix="/api/v1")
app.include_router(ai.router,               prefix="/api/v1")


# ── Root ──────────────────────────────────────────────────────────────────────

@app.get("/")
def root():
    return {
        "name": "Kaveon API",
        "version": "2.0.0",
        "status": "running",
        "endpoints": {
            "health": "/api/health",
            "datasets": "/api/v1/datasets",
            "charts": "/api/v1/charts",
            "dashboards": "/api/v1/dashboards",
        },
    }


@app.api_route("/{path:path}", methods=["GET", "POST", "PUT", "PATCH", "DELETE"])
def not_found(path: str):
    return JSONResponse(
        status_code=404,
        content={"error": {"code": "not_found", "message": f"Route /{path} not found"}},
    )


# ── Dev entrypoint ────────────────────────────────────────────────────────────

if __name__ == "__main__":
    import uvicorn
    print("=" * 44)
    print("Kaveon API")
    print("=" * 44)
    print(f"Server: http://localhost:{settings.API_PORT}")
    print(f"Health: http://localhost:{settings.API_PORT}/api/health")
    print(f"Docs:   http://localhost:{settings.API_PORT}/docs")
    print("=" * 44)
    uvicorn.run("main:app", host="0.0.0.0", port=settings.API_PORT, reload=True)
