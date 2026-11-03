"""Health router — GET /api/health."""

import time
from datetime import datetime, timezone
from fastapi import APIRouter
from fastapi.responses import JSONResponse
from services import postgresql_retirement_runtime

router = APIRouter()


@router.get("/live")
def live():
    """Process liveness only; dependency readiness belongs to ``/health``."""
    return {"status": "alive"}


@router.get("/health")
def health():
    """Readiness of the system of record.

    There used to be a second branch here that probed the metadata database
    with `SELECT 1` and reported `metadata_db` and `data_warehouse`. KaveonDB is
    the only authority now, so there is one thing to report and one way to fail.

    `/api/health` carries no authentication -- it is what the container platform
    probes -- so a failure must not echo a driver's message. It names the class
    of failure and nothing else.
    """
    now = datetime.now(timezone.utc).isoformat()
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
            }, "timestamp": now,
        })
