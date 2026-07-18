"""Setup router — /api/v1/setup + /api/v1/admin/metadata."""

import os
import re
import sys
import threading
from pathlib import Path
from fastapi import APIRouter, Depends, HTTPException
from models.setup import SetupConnectionBody
import database.pool as pool
from middleware.permissions import require_min_role

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


def _read_env_vars() -> dict:
    """
    Read METADATA_* vars from the .env file directly.

    This is the source of truth for "is the app configured?".
    Reading from os.environ is unreliable — a running process retains old values
    after the .env is cleaned, and uvicorn --reload does not create a fresh env.
    Falls back to os.environ only when no .env file exists (Docker / k8s deployments).
    """
    if ENV_PATH.exists():
        content = ENV_PATH.read_text("utf-8")
        def _get(key: str) -> str:
            m = re.search(rf'^\s*{re.escape(key)}\s*=\s*(.+?)\s*$', content, re.MULTILINE)
            return m.group(1).strip().strip("\"'") if m else ""
        return {
            "db_type":  _get("METADATA_DB_TYPE") or "fabric_sql",
            "endpoint": _get("METADATA_ENDPOINT"),
            "database": _get("METADATA_DATABASE"),
            "host":     _get("METADATA_HOST"),
            "port":     _get("METADATA_PORT"),
        }
    # No .env file — deployment via env vars (Docker / k8s)
    return {
        "db_type":  os.environ.get("METADATA_DB_TYPE") or "fabric_sql",
        "endpoint": os.environ.get("METADATA_ENDPOINT") or "",
        "database": os.environ.get("METADATA_DATABASE") or "",
        "host":     os.environ.get("METADATA_HOST") or "",
        "port":     os.environ.get("METADATA_PORT") or "",
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
    # Strip tcp: prefix, port suffix, and ODBC {} value wrappers
    server = server_m.group(1).strip()
    database = db_m.group(1).strip().strip("{}")
    return server, database


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
    )


@router.get("/setup/status")
def setup_status():
    cfg = _read_env_vars()
    endpoint = cfg["endpoint"]
    database = cfg["database"]
    db_type  = cfg["db_type"]

    if not database:
        return {"status": "not_configured"}

    # For MSSQL types we need an endpoint; for others we need a host
    if db_type in ("fabric_sql", "azure_sql") and not endpoint:
        return {"status": "not_configured"}

    # Use the already-warmed connection pool instead of probe_connection, which
    # creates a fresh one-off connection (cold Azure AD auth + TCP handshake every call).
    try:
        result = pool.execute_query(
            "SELECT COUNT(*) AS cnt FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_NAME = 'datasets'",
            database,
        )
        cnt = None
        try:
            cnt = result.get("rows_objects", [{}])[0].get("cnt")
        except (IndexError, TypeError):
            pass
        if cnt is None or int(cnt) == 0:
            return {"status": "schema_missing", "endpoint": endpoint, "database": database}
        return {"status": "ok"}
    except Exception as e:
        err_msg = str(e)
        if "timeout" in err_msg.lower():
            error_type = "timeout"
        elif "login" in err_msg.lower() or "authentication" in err_msg.lower():
            error_type = "auth_failed"
        else:
            error_type = "connection_failed"
        return {
            "status": error_type, "endpoint": endpoint, "database": database,
            "errors": _to_setup_errors(error_type, err_msg),
        }


def _assert_setup_mode():
    """Raise 403 if the app is already fully configured."""
    database = os.environ.get("METADATA_DATABASE")
    db_type = os.environ.get("METADATA_DB_TYPE") or "fabric_sql"
    endpoint = os.environ.get("METADATA_ENDPOINT")
    host = os.environ.get("METADATA_HOST")
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
        "METADATA_DB_TYPE": data.db_type,
        "METADATA_DATABASE": _fab_database,
    }
    if data.db_type in ("fabric_sql", "azure_sql"):
        env_updates["METADATA_ENDPOINT"] = _fab_endpoint
    else:
        env_updates["METADATA_HOST"] = data.host or ""
        env_updates["METADATA_PORT"] = str(data.port or ("5432" if data.db_type == "postgresql" else "3306"))

    try:
        _upsert_env(env_updates)
    except Exception as e:
        print(f"[Setup] Failed to update .env: {e}")

    # Apply to live environment
    for k, v in env_updates.items():
        os.environ[k] = v

    def _restart():
        import time as _t
        import os as _os
        _t.sleep(0.6)
        print("[Setup] Restarting API to apply new metadata database configuration…")
        _os._exit(0)

    threading.Thread(target=_restart, daemon=True).start()

    return {"success": True, "message": "Metadata database initialised successfully. Weft API is restarting…"}


# ── Admin — view / reconfigure metadata server ────────────────────────────────

def _is_metadata_configured() -> bool:
    """Return True if the metadata DB was configured via the UI (.env file)."""
    if ENV_PATH.exists():
        content = ENV_PATH.read_text("utf-8")
        return bool(re.search(r"^\s*METADATA_DATABASE\s*=\s*\S", content, re.MULTILINE))
    return bool(os.environ.get("METADATA_DATABASE"))


def _is_reset_allowed() -> bool:
    """
    Return True if a reset is meaningful — either metadata DB or auth has been
    configured via the UI. Prevents accidental resets on deployments configured
    entirely via OS env vars.

    When a .env file exists it is the sole source of truth — OS env vars are
    intentionally ignored so that a reset (which comments out the keys in the
    file) immediately takes effect on the next restart.
    """
    if ENV_PATH.exists():
        content = ENV_PATH.read_text("utf-8")
        has_metadata = bool(re.search(r"^\s*METADATA_DATABASE\s*=\s*\S", content, re.MULTILINE))
        has_auth     = bool(re.search(r"^\s*AUTH_PROVIDER\s*=\s*\S",     content, re.MULTILINE))
        return has_metadata or has_auth
    return bool(os.environ.get("METADATA_DATABASE") or os.environ.get("AUTH_PROVIDER"))


@router.get("/admin/metadata")
def admin_get_metadata(ctx=Depends(require_min_role("Admin"))):
    """Return current metadata server config (admin only). Never exposes credentials."""
    cfg = _read_env_vars()
    return {
        "db_type":      cfg["db_type"],
        "label":        _DB_TYPE_LABELS.get(cfg["db_type"], cfg["db_type"]),
        "endpoint":     cfg["endpoint"],
        "host":         cfg["host"],
        "port":         cfg["port"],
        "database":     cfg["database"],
        "ui_configured": _is_metadata_configured(),
    }


@router.post("/admin/metadata/test")
def admin_test_metadata(data: SetupConnectionBody, ctx=Depends(require_min_role("Admin"))):
    """Test a new metadata connection without applying it (admin only)."""
    result = _probe(data)
    if result["success"]:
        return {"success": True, "db_type": data.db_type}
    error_type = result.get("error_type", "connection_failed")
    raise HTTPException(
        status_code=400,
        detail={"success": False, "errors": _to_setup_errors(error_type, result.get("message", "Connection failed"))},
    )


@router.post("/admin/metadata/update")
def admin_update_metadata(data: SetupConnectionBody, ctx=Depends(require_min_role("Admin"))):
    """
    Reconfigure the metadata database and restart the API (admin only).
    Unlike /setup/initialize, this works even when the app is already configured.
    """
    schema_path = _SCHEMA_FILES.get(data.db_type)
    if not schema_path or not schema_path.exists():
        raise HTTPException(status_code=500, detail=f"Schema file not found for {_DB_TYPE_LABELS.get(data.db_type, data.db_type)}")

    schema_sql = schema_path.read_text("utf-8")
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

    if data.db_type == "fabric_sql":
        _fab_endpoint, _fab_database = _resolve_fabric(data)
    else:
        _fab_endpoint, _fab_database = (data.endpoint or ""), (data.database or "")

    env_updates = {"METADATA_DB_TYPE": data.db_type, "METADATA_DATABASE": _fab_database}
    if data.db_type in ("fabric_sql", "azure_sql"):
        env_updates["METADATA_ENDPOINT"] = _fab_endpoint
    else:
        env_updates["METADATA_HOST"] = data.host or ""
        env_updates["METADATA_PORT"] = str(data.port or ("5432" if data.db_type == "postgresql" else "3306"))

    try:
        _upsert_env(env_updates)
    except Exception as e:
        print(f"[Setup] Failed to update .env: {e}")

    for k, v in env_updates.items():
        os.environ[k] = v

    def _restart():
        import time as _t
        import os as _os
        _t.sleep(0.6)
        print("[Setup] Restarting API after admin metadata reconfiguration…")
        _os._exit(0)

    threading.Thread(target=_restart, daemon=True).start()
    return {"success": True, "message": "Metadata database updated. Weft API is restarting…"}


# ── Start Fresh ───────────────────────────────────────────────────────────────

@router.post("/admin/reset")
def admin_start_fresh(ctx=Depends(require_min_role("Admin"))):
    """
    Reset Weft to a fresh state: clears the metadata database configuration
    and auth config, then restarts the API so the setup wizard is shown again.

    Only available when the metadata DB was configured via the UI (not via
    deployment env vars), to prevent accidental resets on managed deployments.
    """
    if not _is_reset_allowed():
        raise HTTPException(
            status_code=403,
            detail={
                "code": "deployment_configured",
                "message": (
                    "Start Fresh is only available when Weft was configured "
                    "through the UI. This instance appears to be configured via "
                    "deployment environment variables."
                ),
            },
        )

    # Comment out METADATA_* and AUTH_* keys from the .env file
    _KEYS_TO_CLEAR = [
        "METADATA_DB_TYPE", "METADATA_DATABASE", "METADATA_ENDPOINT",
        "METADATA_HOST", "METADATA_PORT",
        "AUTH_PROVIDER", "AUTH_AZURE_TENANT_ID", "AUTH_AZURE_CLIENT_ID",
        "AUTH_GOOGLE_CLIENT_ID", "AUTH_GOOGLE_CLIENT_SECRET",
    ]
    try:
        if ENV_PATH.exists():
            content = ENV_PATH.read_text("utf-8")
            for key in _KEYS_TO_CLEAR:
                content = re.sub(
                    rf"^(\s*){re.escape(key)}\s*=.*$",
                    rf"\g<1># {key}=",
                    content,
                    flags=re.MULTILINE,
                )
            ENV_PATH.write_text(content, "utf-8")
    except Exception as e:
        print(f"[Setup] start-fresh: failed to update .env: {e}")

    # Clear from the live process environment
    for key in _KEYS_TO_CLEAR:
        os.environ.pop(key, None)

    def _restart():
        import time as _t
        import os as _os
        _t.sleep(0.4)
        print("[Setup] Restarting API after Start Fresh…")
        _os._exit(0)

    threading.Thread(target=_restart, daemon=True).start()
    return {"success": True, "message": "Configuration cleared. Weft is restarting…"}
