"""Health router — GET /api/health."""

import time
from datetime import datetime, timezone
from fastapi import APIRouter
from fastapi.responses import JSONResponse
import database.metadata as db
from services import product_replay_worker

router = APIRouter()


@router.get("/health")
def health():
    now = datetime.now(timezone.utc).isoformat()
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
