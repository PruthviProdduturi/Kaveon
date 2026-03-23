"""Auth config service — reads/writes auth config in the .env file.

Auth configuration is stored in the .env file, independent of the metadata
database. This allows authentication to be configured before (or without) a
metadata database being present.

Env keys:
    AUTH_PROVIDER               — azure_ad | google | local  (default: local)
    AUTH_AZURE_TENANT_ID        — Azure AD tenant UUID
    AUTH_AZURE_CLIENT_ID        — Azure AD app client UUID
    AUTH_GOOGLE_CLIENT_ID       — Google OAuth2 client ID
    AUTH_GOOGLE_CLIENT_SECRET   — Fernet-encrypted Google secret
    AUTH_JWT_SECRET             — Fernet-encrypted JWT signing secret
"""

import base64
import hashlib
import os
import re
import secrets
from pathlib import Path
from typing import Optional

from config import settings

_REPO_ROOT = Path(__file__).resolve().parents[1]
ENV_PATH = _REPO_ROOT.parent.parent / ".env"

# ── Fernet key derivation ──────────────────────────────────────────────────────

def _fernet_key() -> bytes:
    secret = settings.AI_ENCRYPTION_SECRET or "loomx-default-encryption-secret"
    return base64.urlsafe_b64encode(hashlib.sha256(secret.encode()).digest())


def _encrypt(plaintext: str) -> str:
    from cryptography.fernet import Fernet
    return Fernet(_fernet_key()).encrypt(plaintext.encode()).decode()


def _decrypt(ciphertext: str) -> str:
    from cryptography.fernet import Fernet
    return Fernet(_fernet_key()).decrypt(ciphertext.encode()).decode()


# ── .env helpers ───────────────────────────────────────────────────────────────

def _read_key(key: str) -> str:
    """Read a key from .env file first, then os.environ as fallback.
    An explicitly empty value (KEY=) in .env takes precedence over os.environ."""
    if ENV_PATH.exists():
        for line in ENV_PATH.read_text("utf-8").splitlines():
            stripped = line.strip()
            if stripped.startswith("#") or "=" not in stripped:
                continue
            k, _, v = stripped.partition("=")
            if k.strip() == key:
                return v.strip().strip("\"'")
    return os.environ.get(key, "")


def _upsert_env(updates: dict) -> None:
    """Write key=value pairs into the .env file (creates it if absent)."""
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


# ── Module-level cache ─────────────────────────────────────────────────────────

_cached_provider: Optional[str] = None


def refresh_auth_config() -> None:
    """Clear the cached provider so the next read re-reads from .env."""
    global _cached_provider
    _cached_provider = None


# ── Public API ─────────────────────────────────────────────────────────────────

def get_active_provider() -> str:
    """Return the active auth provider. Defaults to 'local'."""
    global _cached_provider
    if _cached_provider is not None:
        return _cached_provider
    _cached_provider = _read_key("AUTH_PROVIDER") or "local"
    return _cached_provider


def get_config() -> dict:
    """Return full auth config. Secrets are masked."""
    return {
        "provider":            _read_key("AUTH_PROVIDER") or "local",
        "azure_tenant_id":     _read_key("AUTH_AZURE_TENANT_ID"),
        "azure_client_id":     _read_key("AUTH_AZURE_CLIENT_ID"),
        "google_client_id":    _read_key("AUTH_GOOGLE_CLIENT_ID"),
        "google_client_secret": "***" if _read_key("AUTH_GOOGLE_CLIENT_SECRET") else "",
        "jwt_secret":           "***" if _read_key("AUTH_JWT_SECRET") else "",
    }


def upsert_config(data: dict) -> dict:
    """Write auth config to .env and update the live process environment."""
    provider = data.get("provider", "local")
    updates: dict = {"AUTH_PROVIDER": provider}

    if provider == "azure_ad":
        if data.get("azure_tenant_id"):
            updates["AUTH_AZURE_TENANT_ID"] = data["azure_tenant_id"]
        if data.get("azure_client_id"):
            updates["AUTH_AZURE_CLIENT_ID"] = data["azure_client_id"]
    elif provider == "google":
        if data.get("google_client_id"):
            updates["AUTH_GOOGLE_CLIENT_ID"] = data["google_client_id"]
        raw_secret = data.get("google_client_secret", "")
        if raw_secret and raw_secret != "***":
            updates["AUTH_GOOGLE_CLIENT_SECRET"] = _encrypt(raw_secret)

    _upsert_env(updates)
    for k, v in updates.items():
        os.environ[k] = v

    refresh_auth_config()
    return get_config()


LOOMX_APP_ROLES = [
    {
        "id": "7f6e4b2a-1c3d-4e5f-8a9b-0d1e2f3c4b5a",
        "displayName": "Viewer",
        "value": "Viewer",
        "description": "Read-only access to published content",
        "allowedMemberTypes": ["User"],
        "isEnabled": True,
    },
    {
        "id": "2b3c4d5e-6f7a-8b9c-0d1e-2f3a4b5c6d7e",
        "displayName": "Analyst",
        "value": "Analyst",
        "description": "Create and edit own content, SQL Lab access",
        "allowedMemberTypes": ["User"],
        "isEnabled": True,
    },
    {
        "id": "9a8b7c6d-5e4f-3a2b-1c0d-ef9a8b7c6d5e",
        "displayName": "Editor",
        "value": "Editor",
        "description": "Create, edit and publish any content",
        "allowedMemberTypes": ["User"],
        "isEnabled": True,
    },
    {
        "id": "1a2b3c4d-5e6f-7a8b-9c0d-1e2f3a4b5c6d",
        "displayName": "Admin",
        "value": "Admin",
        "description": "Full access including user and data source management",
        "allowedMemberTypes": ["User"],
        "isEnabled": True,
    },
]


def setup_azure_app_roles(graph_token: str, client_id: str, tenant_id: str) -> dict:
    """
    Create the 4 LoomX App Roles in the Azure AD App Registration using a
    delegated Microsoft Graph token supplied by the caller.

    The token must have Application.ReadWrite.OwnedBy (or .All) scope.
    Raises PermissionError on 403, ValueError on other API errors.
    Idempotent — existing roles are skipped.
    """
    import json as _json
    import urllib.error
    import urllib.request

    headers = {"Authorization": f"Bearer {graph_token}"}

    # ── 1. Resolve the application object ID ──────────────────────────────────
    search_url = (
        f"https://graph.microsoft.com/v1.0/applications"
        f"?$filter=appId eq '{client_id}'&$select=id,appId,appRoles"
    )
    req = urllib.request.Request(search_url, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=15) as resp:
            apps_data = _json.loads(resp.read())
    except urllib.error.HTTPError as e:
        body = e.read().decode("utf-8", errors="replace")
        if e.code == 401:
            raise PermissionError(
                "Graph API token is invalid or expired. "
                "Re-open the modal and try again to get a fresh token."
            )
        if e.code == 403:
            raise PermissionError(
                "You don't have permission to read App Registrations in this tenant. "
                "You need to be an Owner of the App Registration to auto-create roles. "
                "Ask your Azure AD admin, or copy the manifest below and add the roles manually."
            )
        raise ValueError(f"Failed to query app registrations ({e.code}): {body}")

    app_list = apps_data.get("value", [])
    if not app_list:
        raise ValueError(
            f"No app registration found for client_id '{client_id}' in tenant '{tenant_id}'. "
            "Verify the Client ID is correct."
        )

    app            = app_list[0]
    object_id      = app["id"]
    existing_roles = app.get("appRoles", [])
    existing_ids   = {r["id"]    for r in existing_roles}
    existing_vals  = {r["value"] for r in existing_roles}

    # ── 2. Merge: keep existing, append only missing LoomX roles ──────────────
    created: list[str] = []
    skipped: list[str] = []
    merged = list(existing_roles)

    for role in LOOMX_APP_ROLES:
        if role["id"] in existing_ids or role["value"] in existing_vals:
            skipped.append(role["value"])
        else:
            merged.append(role)
            created.append(role["value"])

    if not created:
        return {
            "success": True,
            "created": [],
            "skipped": skipped,
            "message": "All LoomX App Roles already exist in the App Registration.",
        }

    # ── 3. PATCH the app registration ─────────────────────────────────────────
    patch_url  = f"https://graph.microsoft.com/v1.0/applications/{object_id}"
    patch_body = _json.dumps({"appRoles": merged}).encode()
    req2 = urllib.request.Request(patch_url, data=patch_body, method="PATCH", headers={
        **headers, "Content-Type": "application/json",
    })
    try:
        with urllib.request.urlopen(req2, timeout=15) as resp:
            resp.read()
    except urllib.error.HTTPError as e:
        body = e.read().decode("utf-8", errors="replace")
        if e.code == 403:
            raise PermissionError(
                "You don't have permission to modify App Roles on this App Registration. "
                "You must be an Owner of the app registration. "
                "Ask your Azure AD admin to grant you Owner access, or copy the manifest below "
                "and ask them to add the roles manually."
            )
        try:
            msg = _json.loads(body).get("error", {}).get("message", body)
        except Exception:
            msg = body
        raise ValueError(f"Failed to update App Roles ({e.code}): {msg}")

    return {
        "success": True,
        "created": created,
        "skipped": skipped,
        "message": (
            f"Created {len(created)} App Role(s): {', '.join(created)}. "
            "Assign users in Azure AD → Enterprise Applications → your app → Users and groups."
        ),
    }


def get_jwt_secret() -> str:
    """
    Return the JWT signing secret.
    Generates, encrypts, and persists a new secret if one doesn't exist yet.
    Falls back to a deterministic derived secret if the .env is not writable.
    """
    raw = _read_key("AUTH_JWT_SECRET")
    if raw:
        try:
            return _decrypt(raw)
        except Exception as e:
            print(f"[AuthConfig] jwt_secret decrypt failed — regenerating: {e}")

    new_secret = secrets.token_hex(32)
    try:
        enc = _encrypt(new_secret)
        _upsert_env({"AUTH_JWT_SECRET": enc})
        os.environ["AUTH_JWT_SECRET"] = enc
    except Exception as e:
        print(f"[AuthConfig] Could not persist jwt_secret to .env: {e}")

    return new_secret
