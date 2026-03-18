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

_REPO_ROOT = Path(__file__).resolve().parents[1]
ENV_PATH = _REPO_ROOT.parent.parent / ".env"

_SCHEMA_FILES = {
    "fabric_sql": _REPO_ROOT / "schema.sql",
    "azure_sql":  _REPO_ROOT / "schema.sql",
    "postgresql": _REPO_ROOT / "schema_postgresql.sql",
    "mysql":      _REPO_ROOT / "schema_mysql.sql",
}

_ISSUE_CODES = {
    "connection_failed": [
        {"code": 1007, "message": "The hostname / endpoint cannot be resolved — check for typos."},
        {"code": 1008, "message": "The required port may be blocked by a firewall or VPN."},
    ],
    "timeout": [
        {"code": 1008, "message": "Connection timed out — the host port is unreachable from this machine."},
    ],
    "access_denied": [
        {"code": 1017, "message": "Authentication failed — verify credentials or Azure AD role assignments."},
    ],
    "db_not_found": [
        {"code": 1015, "message": "The database was not found. Names are case-sensitive — copy the exact name."},
    ],
}

_DB_TYPE_LABELS = {
    "fabric_sql": "Fabric SQL",
    "azure_sql":  "Azure SQL",
    "postgresql": "PostgreSQL",
    "mysql":      "MySQL",
}


def _to_setup_errors(error_type: str, message: str) -> list:
    return [{
        "message": message, "error_type": error_type, "level": "error",
        "extra": {"issue_codes": _ISSUE_CODES.get(error_type, [{"code": 0, "message": "An unexpected error occurred."}])},
    }]


def _upsert_env(updates: dict):
    content = ENV_PATH.read_text("utf-8") if ENV_PATH.exists() else ""
    for key, value in updates.items():
        safe = f'"{value}"' if re.search(r'[\s#"\']', str(value)) else str(value)
        line = f"{key}={safe}"
        pattern = re.compile(rf"^(\s*#?\s*){re.escape(key)}\s*=.*$", re.MULTILINE)
        if pattern.search(content):
            content = pattern.sub(line, content)
        else:
            content = content.rstrip() + "\n" + line + "\n"
    ENV_PATH.write_text(content, "utf-8")


def _parse_odbc(conn_str: str) -> tuple:
    """
    Extract (server, database) from a standard ODBC connection string.
    Handles both 'Server=tcp:host,port' and 'Server=host' forms.
    Returns (endpoint, database) — endpoint has tcp: prefix and port stripped.
    """
    server_m = re.search(
        r'(?:Server|Data Source)\s*=\s*(?:tcp:)?([^,;\s]+)',
        conn_str, re.IGNORECASE,
    )
    db_m = re.search(
        r'(?:Initial Catalog|Database)\s*=\s*([^;]+)',
        conn_str, re.IGNORECASE,
    )
    if not server_m or not db_m:
        raise ValueError(
            "Connection string must include 'Server' (or 'Data Source') "
            "and 'Initial Catalog' (or 'Database')."
        )
    return server_m.group(1).strip(), db_m.group(1).strip()


def _resolve_fabric(data: SetupConnectionBody) -> tuple:
    """
    For Fabric SQL: parse the ODBC connection string if provided,
    otherwise fall back to explicit endpoint + database.
    Returns (endpoint, database).
    """
    if data.connection_string and data.connection_string.strip():
        return _parse_odbc(data.connection_string)
    return (data.endpoint or ""), (data.database or "")


def _probe(data: SetupConnectionBody, statements=None):
    """Call pool.probe_connection with the right params for the given db_type."""
    if data.db_type == "fabric_sql":
        endpoint, database = _resolve_fabric(data)
    else:
        endpoint, database = (data.endpoint or ""), (data.database or "")

    return pool.probe_connection(
        endpoint=endpoint,
        database=database,
        statements=statements,
        db_type=data.db_type,
        host=data.host or "",
        port=data.port or 0,
        username=data.username or "",
        password=data.password or "",
    )


@router.get("/setup/status")
def setup_status():
    endpoint = os.environ.get("FABRIC_METADATA_ENDPOINT")
    database = os.environ.get("FABRIC_METADATA_DATABASE")
    db_type = os.environ.get("FABRIC_METADATA_DB_TYPE") or "fabric_sql"

    if not database:
        return {"status": "not_configured"}

    # For MSSQL types we need an endpoint; for others we need a host
    if db_type in ("fabric_sql", "azure_sql") and not endpoint:
        return {"status": "not_configured"}

    result = pool.probe_connection(
        endpoint=endpoint or "",
        database=database,
        statements=[
            "SELECT 1 AS connection_test",
            "SELECT COUNT(*) AS cnt FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_NAME = 'datasets'",
        ],
        db_type=db_type,
        host=os.environ.get("FABRIC_METADATA_HOST") or "",
        port=int(os.environ.get("FABRIC_METADATA_PORT") or 0),
        username=os.environ.get("FABRIC_METADATA_USERNAME") or "",
        password=os.environ.get("FABRIC_METADATA_PASSWORD") or "",
    )

    if not result["success"]:
        error_type = result.get("error_type", "connection_failed")
        return {
            "status": error_type, "endpoint": endpoint, "database": database,
            "errors": _to_setup_errors(error_type, result.get("message", "Connection failed")),
        }

    cnt = None
    try:
        cnt = result.get("results", [{}])[1].get("rows", [[None]])[0][0]
    except (IndexError, TypeError):
        pass
    if cnt is None or int(cnt) == 0:
        return {"status": "schema_missing", "endpoint": endpoint, "database": database}

    return {"status": "ok"}


def _assert_setup_mode():
    """Raise 403 if the app is already fully configured."""
    database = os.environ.get("FABRIC_METADATA_DATABASE")
    db_type = os.environ.get("FABRIC_METADATA_DB_TYPE") or "fabric_sql"
    endpoint = os.environ.get("FABRIC_METADATA_ENDPOINT")
    host = os.environ.get("FABRIC_METADATA_HOST")
    if database and (endpoint if db_type in ("fabric_sql", "azure_sql") else host):
        raise HTTPException(
            status_code=403,
            detail="Setup endpoints are disabled once the application is configured.",
        )


@router.post("/setup/test")
def setup_test(data: SetupConnectionBody):
    _assert_setup_mode()
    result = _probe(data)
    if result["success"]:
        return {"success": True, "db_type": data.db_type}
    error_type = result.get("error_type", "connection_failed")
    raise HTTPException(
        status_code=400,
        detail={"success": False, "errors": _to_setup_errors(error_type, result.get("message", "Connection failed"))},
    )


@router.post("/setup/initialize")
def setup_initialize(data: SetupConnectionBody):
    _assert_setup_mode()

    schema_path = _SCHEMA_FILES.get(data.db_type)
    if not schema_path or not schema_path.exists():
        raise HTTPException(status_code=500, detail=f"Schema file not found for {_DB_TYPE_LABELS.get(data.db_type, data.db_type)}")

    schema_sql = schema_path.read_text("utf-8")

    # Split on GO (T-SQL) for MSSQL; split on ; for others
    if data.db_type in ("fabric_sql", "azure_sql"):
        batches = [b.strip() for b in re.split(r"^\s*GO\s*$", schema_sql, flags=re.MULTILINE | re.IGNORECASE) if b.strip()]
    else:
        batches = [b.strip() for b in schema_sql.split(";") if b.strip()]

    result = _probe(data, batches)
    if not result["success"]:
        error_type = result.get("error_type", "connection_failed")
        raise HTTPException(
            status_code=400,
            detail={"success": False, "errors": _to_setup_errors(error_type, result.get("message", "Schema initialisation failed"))},
        )

    # Persist to .env
    if data.db_type == "fabric_sql":
        _fab_endpoint, _fab_database = _resolve_fabric(data)
    else:
        _fab_endpoint, _fab_database = (data.endpoint or ""), (data.database or "")

    env_updates = {
        "FABRIC_METADATA_DB_TYPE": data.db_type,
        "FABRIC_METADATA_DATABASE": _fab_database,
    }
    if data.db_type in ("fabric_sql", "azure_sql"):
        env_updates["FABRIC_METADATA_ENDPOINT"] = _fab_endpoint
    else:
        env_updates["FABRIC_METADATA_HOST"] = data.host or ""
        env_updates["FABRIC_METADATA_PORT"] = str(data.port or ("5432" if data.db_type == "postgresql" else "3306"))
        env_updates["FABRIC_METADATA_USERNAME"] = data.username or ""
        env_updates["FABRIC_METADATA_PASSWORD"] = data.password or ""

    try:
        _upsert_env(env_updates)
    except Exception as e:
        print(f"[Setup] Failed to update .env: {e}")

    # Apply to live environment
    for k, v in env_updates.items():
        os.environ[k] = v

    def _restart():
        import time as _t
        _t.sleep(0.6)
        print("[Setup] Restarting API to apply new metadata database configuration…")
        sys.exit(0)

    threading.Thread(target=_restart, daemon=True).start()

    return {"success": True, "message": "Metadata database initialised successfully. LoomX API is restarting…"}
