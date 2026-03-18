"""Setup router — /api/v1/setup."""

import os
import re
import sys
import threading
from pathlib import Path
from fastapi import APIRouter, HTTPException
from models.setup import SetupConnectionBody
import database.pool as pool

router = APIRouter()

_REPO_ROOT = Path(__file__).resolve().parents[2]
ENV_PATH = _REPO_ROOT.parent.parent / ".env"
SCHEMA_PATH = Path(__file__).resolve().parents[1] / "schema.sql"

_ISSUE_CODES = {
    "connection_failed": [
        {"code": 1007, "message": "The hostname provided cannot be resolved — check for typos in the endpoint."},
        {"code": 1008, "message": "Port 1433 (required by Fabric SQL) may be blocked by a firewall or VPN."},
    ],
    "timeout": [
        {"code": 1008, "message": "Connection timed out — port 1433 is unreachable from this machine."},
    ],
    "access_denied": [
        {"code": 1017, "message": "The service identity lacks permission. Grant the Contributor or Member role in the Fabric workspace."},
    ],
    "db_not_found": [
        {"code": 1015, "message": "The database was not found. Database names are case-sensitive in Fabric SQL."},
    ],
}


def _to_setup_errors(error_type: str, message: str) -> list:
    return [{
        "message": message, "error_type": error_type, "level": "error",
        "extra": {"issue_codes": _ISSUE_CODES.get(error_type, [{"code": 0, "message": "An unexpected error occurred."}])},
    }]


def _upsert_env(updates: dict):
    content = ENV_PATH.read_text("utf-8") if ENV_PATH.exists() else ""
    for key, value in updates.items():
        safe = f'"{value}"' if re.search(r'[\s#"\']', value) else value
        line = f"{key}={safe}"
        pattern = re.compile(rf"^(\s*#?\s*){re.escape(key)}\s*=.*$", re.MULTILINE)
        if pattern.search(content):
            content = pattern.sub(line, content)
        else:
            content = content.rstrip() + "\n" + line + "\n"
    ENV_PATH.write_text(content, "utf-8")


@router.get("/setup/status")
def setup_status():
    endpoint = os.environ.get("FABRIC_METADATA_ENDPOINT")
    database = os.environ.get("FABRIC_METADATA_DATABASE")

    if not endpoint or not database:
        return {"status": "not_configured"}

    result = pool.probe_connection(endpoint, database, [
        "SELECT 1 AS connection_test",
        "SELECT COUNT(*) AS cnt FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_NAME = 'datasets'",
    ])

    if not result["success"]:
        error_type = result.get("error_type", "connection_failed")
        return {
            "status": error_type, "endpoint": endpoint, "database": database,
            "errors": _to_setup_errors(error_type, result.get("message", "Connection failed")),
        }

    # Check schema
    cnt = None
    try:
        cnt = result.get("results", [{}])[1].get("rows", [[None]])[0][0]
    except (IndexError, TypeError):
        pass
    if cnt is None or int(cnt) == 0:
        return {"status": "schema_missing", "endpoint": endpoint, "database": database}

    return {"status": "ok"}


def _assert_setup_mode():
    """Raise 403 if the app is already fully configured — setup endpoints must not be callable post-config."""
    endpoint = os.environ.get("FABRIC_METADATA_ENDPOINT")
    database = os.environ.get("FABRIC_METADATA_DATABASE")
    if endpoint and database:
        raise HTTPException(
            status_code=403,
            detail="Setup endpoints are disabled once the application is configured.",
        )


@router.post("/setup/test")
def setup_test(data: SetupConnectionBody):
    _assert_setup_mode()
    result = pool.probe_connection(data.endpoint, data.database)
    if result["success"]:
        return {"success": True}
    error_type = result.get("error_type", "connection_failed")
    raise HTTPException(
        status_code=400,
        detail={"success": False, "errors": _to_setup_errors(error_type, result.get("message", "Connection failed"))},
    )


@router.post("/setup/initialize")
def setup_initialize(data: SetupConnectionBody):
    _assert_setup_mode()
    endpoint = data.endpoint
    database = data.database

    if not SCHEMA_PATH.exists():
        raise HTTPException(status_code=500, detail="schema.sql not found on the server")

    schema_sql = SCHEMA_PATH.read_text("utf-8")
    batches = [b.strip() for b in re.split(r"^\s*GO\s*$", schema_sql, flags=re.MULTILINE | re.IGNORECASE) if b.strip()]

    result = pool.probe_connection(endpoint, database, batches)
    if not result["success"]:
        error_type = result.get("error_type", "connection_failed")
        raise HTTPException(
            status_code=400,
            detail={"success": False, "errors": _to_setup_errors(error_type, result.get("message", "Schema initialisation failed"))},
        )

    try:
        _upsert_env({"FABRIC_METADATA_ENDPOINT": endpoint, "FABRIC_METADATA_DATABASE": database})
    except Exception as e:
        print(f"[Setup] Failed to update .env: {e}")

    os.environ["FABRIC_METADATA_ENDPOINT"] = endpoint
    os.environ["FABRIC_METADATA_DATABASE"] = database

    def _restart():
        import time
        time.sleep(0.6)
        print("[Setup] Restarting API to apply new metadata database configuration…")
        sys.exit(0)

    threading.Thread(target=_restart, daemon=True).start()

    return {"success": True, "message": "Metadata database initialised successfully. LoomX API is restarting…"}
