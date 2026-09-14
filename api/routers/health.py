"""Health router — GET /api/health."""

import time
from datetime import datetime, timezone
from fastapi import APIRouter
from fastapi.responses import JSONResponse
import database.metadata as db
from services import product_replay_worker
from services import postgresql_retirement_runtime

router = APIRouter()


@router.get("/health")
def health():
    now = datetime.now(timezone.utc).isoformat()
    if postgresql_retirement_runtime.requested():
        try:
            t0 = time.monotonic()
            state = postgresql_retirement_runtime.probe()
            latency_ms = int((time.monotonic() - t0) * 1000)
            return {
                "status": "healthy", "authority": "kaveondb",
                "checks": {
                    "kaveondb": {"connected": True, "authoritative": True,
                                  "authority_family_count": state["authority_family_count"],
                                  "latency_ms": latency_ms, "last_check": now},
                    "postgresql": {"connected": False, "required": False, "last_check": now},
                    "azure_ad": {"connected": True, "latency_ms": 0, "last_check": now},
                },
                "timestamp": now,
            }
        except Exception:
            return JSONResponse(status_code=503, content={
                "status": "degraded", "authority": "kaveondb",
                "checks": {
                    "kaveondb": {"connected": False, "authoritative": True,
                                  "message": "KaveonDB authority validation failed",
                                  "latency_ms": 0, "last_check": now},
                    "postgresql": {"connected": False, "required": False, "last_check": now},
                }, "timestamp": now,
            })
    try:
        t0 = time.monotonic()
        db.query("SELECT 1 AS test")
        latency_ms = int((time.monotonic() - t0) * 1000)
        return {
            "status": "healthy",
            "checks": {
                "metadata_db": {"connected": True, "latency_ms": latency_ms, "last_check": now},
                "data_warehouse": {"connected": True, "latency_ms": 0, "last_check": now},
                "azure_ad": {"connected": True, "latency_ms": 0, "last_check": now},
                "product_replay": product_replay_worker.status(),
            },
            "timestamp": now,
        }
    except Exception as e:
        return JSONResponse(
            status_code=503,
            content={
                "status": "degraded",
                "checks": {
                    "metadata_db": {"connected": False, "message": str(e), "latency_ms": 0, "last_check": now},
                    "data_warehouse": {"connected": True, "latency_ms": 0, "last_check": now},
                    "azure_ad": {"connected": True, "latency_ms": 0, "last_check": now},
                },
                "timestamp": now,
            },
        )
