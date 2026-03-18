"""
LoomX API — FastAPI entry point.
Replaces the Node.js/Express API + Python Flask proxy with a single service.
"""

import threading
from contextlib import asynccontextmanager

from fastapi import FastAPI, Request
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import JSONResponse
import time

from config import settings
from middleware.errors import AppError, app_error_handler, generic_error_handler
from database.pool import LargeIntResponse
from database.warmup import start_warmup_and_heartbeat

# Routers
from routers import (
    auth,
    health,
    datasets,
    charts,
    dashboards,
    metadata_summary,
    data_sources,
    favorites,
    lab,
    sql,
    theme,
    setup,
)


# ── Lifespan ──────────────────────────────────────────────────────────────────

@asynccontextmanager
async def lifespan(app: FastAPI):
    # Startup: kick off pool warmup + heartbeat in the background
    if settings.METADATA_ENDPOINT and settings.METADATA_DATABASE:
        threading.Thread(target=start_warmup_and_heartbeat, daemon=True).start()
        print("[API] Connection pool warmup started.")
    else:
        print("[API] WARNING: METADATA_ENDPOINT / METADATA_DATABASE not set.")
        print("[API] Starting in setup mode — /api/v1/setup/status is available.")
    yield
    # Shutdown: pools close via GC; nothing explicit needed


# ── App ───────────────────────────────────────────────────────────────────────

app = FastAPI(
    title="LoomX API",
    version="2.0.0",
    default_response_class=LargeIntResponse,
    lifespan=lifespan,
)

# CORS — only the known frontend origin; explicit method list (no wildcard)
app.add_middleware(
    CORSMiddleware,
    allow_origins=[settings.WEB_URL],
    allow_credentials=True,
    allow_methods=["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"],
    allow_headers=["Authorization", "Content-Type", "x-user-email", "Cache-Control"],
)

# Exception handlers
app.add_exception_handler(AppError, app_error_handler)
app.add_exception_handler(Exception, generic_error_handler)


# ── Security headers ──────────────────────────────────────────────────────────

@app.middleware("http")
async def security_headers(request: Request, call_next):
    response = await call_next(request)
    response.headers["X-Content-Type-Options"] = "nosniff"
    response.headers["X-Frame-Options"] = "DENY"
    response.headers["Strict-Transport-Security"] = "max-age=31536000; includeSubDomains"
    response.headers["Referrer-Policy"] = "strict-origin-when-cross-origin"
    response.headers["X-Permitted-Cross-Domain-Policies"] = "none"
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


# ── Root ──────────────────────────────────────────────────────────────────────

@app.get("/")
def root():
    return {
        "name": "LoomX v2 API",
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
    print("LoomX API")
    print("=" * 44)
    print(f"Server: http://localhost:{settings.API_PORT}")
    print(f"Health: http://localhost:{settings.API_PORT}/api/health")
    print(f"Docs:   http://localhost:{settings.API_PORT}/docs")
    print("=" * 44)
    uvicorn.run("main:app", host="0.0.0.0", port=settings.API_PORT, reload=True)
