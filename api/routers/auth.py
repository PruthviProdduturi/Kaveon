"""Auth router — POST /api/connect, POST /api/disconnect."""

from datetime import datetime, timezone
from fastapi import APIRouter
from middleware.demo import allowed_in_demo

router = APIRouter()


@router.post("/connect")
@allowed_in_demo
def connect():
    """
    Called by the frontend immediately after Azure AD authentication.
    Pool warmup is already running in the background (started at lifespan).
    """
    return {
        "success": True,
        "message": "Connected successfully",
        "timestamp": datetime.now(timezone.utc).isoformat(),
    }


@router.post("/disconnect")
@allowed_in_demo
def disconnect():
    return {
        "success": True,
        "message": "Disconnected successfully",
        "timestamp": datetime.now(timezone.utc).isoformat(),
    }
