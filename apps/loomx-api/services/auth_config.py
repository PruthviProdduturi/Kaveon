"""Auth config service — read/write auth_config table, secret encryption."""

import base64
import hashlib
import os
import secrets
from datetime import datetime, timezone
from typing import Optional

import database.metadata as db
from config import settings

# ── Fernet key derivation ─────────────────────────────────────────────────────

def _fernet_key() -> bytes:
    """Derive a 32-byte Fernet key from AI_ENCRYPTION_SECRET (or a fixed fallback)."""
    secret = settings.AI_ENCRYPTION_SECRET or "loomx-default-encryption-secret"
    return base64.urlsafe_b64encode(hashlib.sha256(secret.encode()).digest())


def _encrypt(plaintext: str) -> str:
    from cryptography.fernet import Fernet
    f = Fernet(_fernet_key())
    return f.encrypt(plaintext.encode()).decode()


def _decrypt(ciphertext: str) -> str:
    from cryptography.fernet import Fernet
    f = Fernet(_fernet_key())
    return f.decrypt(ciphertext.encode()).decode()


# ── Module-level cache (cleared by refresh_auth_config) ──────────────────────

_cached_provider: Optional[str] = None


def refresh_auth_config() -> None:
    """Clear cached provider so the next request re-reads from DB."""
    global _cached_provider
    _cached_provider = None


# ── Public API ────────────────────────────────────────────────────────────────

def get_active_provider() -> str:
    """
    Return the active auth provider name.
    Returns 'local' if DB is unavailable or no row exists yet.
    """
    global _cached_provider
    if _cached_provider is not None:
        return _cached_provider

    try:
        row = db.query_one("SELECT TOP 1 provider FROM auth_config ORDER BY id DESC")
        _cached_provider = row["provider"] if row else "local"
    except Exception as e:
        print(f"[AuthConfig] get_active_provider failed — falling back to 'local': {e}")
        _cached_provider = "local"

    return _cached_provider


def get_config() -> dict:
    """
    Return full auth config.  Secrets are masked in the returned dict.
    """
    try:
        row = db.query_one(
            "SELECT TOP 1 id, provider, azure_tenant_id, azure_client_id, "
            "google_client_id, google_client_secret, jwt_secret, "
            "updated_at, updated_by FROM auth_config ORDER BY id DESC"
        )
    except Exception as e:
        print(f"[AuthConfig] get_config failed: {e}")
        return {"provider": "local"}

    if not row:
        return {"provider": "local"}

    return {
        "id": row.get("id"),
        "provider": row.get("provider", "local"),
        "azure_tenant_id": row.get("azure_tenant_id") or "",
        "azure_client_id": row.get("azure_client_id") or "",
        "google_client_id": row.get("google_client_id") or "",
        # Secrets are masked — never exposed via API
        "google_client_secret": "***" if row.get("google_client_secret") else "",
        "jwt_secret": "***" if row.get("jwt_secret") else "",
        "updated_at": row.get("updated_at").isoformat() if row.get("updated_at") else None,
        "updated_by": row.get("updated_by") or "",
    }


def upsert_config(data: dict) -> dict:
    """
    Write (or overwrite) the single auth_config row.
    google_client_secret and jwt_secret are encrypted before storage.
    Returns the saved config (secrets masked).
    """
    provider = data.get("provider", "local")
    azure_tenant_id = data.get("azure_tenant_id") or None
    azure_client_id = data.get("azure_client_id") or None
    google_client_id = data.get("google_client_id") or None
    now = datetime.now(timezone.utc).replace(tzinfo=None)
    updated_by = data.get("updated_by") or "admin"

    # Encrypt google_client_secret if provided (and not masked)
    raw_google_secret = data.get("google_client_secret", "")
    if raw_google_secret and raw_google_secret != "***":
        enc_google_secret = _encrypt(raw_google_secret)
    else:
        # Preserve existing value — fetch from DB
        existing = _get_raw_row()
        enc_google_secret = existing.get("google_client_secret") if existing else None

    # Encrypt jwt_secret if provided (and not masked)
    raw_jwt_secret = data.get("jwt_secret", "")
    if raw_jwt_secret and raw_jwt_secret != "***":
        enc_jwt_secret = _encrypt(raw_jwt_secret)
    else:
        existing = existing if "existing" in dir() else _get_raw_row()
        enc_jwt_secret = existing.get("jwt_secret") if existing else None

    # Delete-insert pattern — single row table
    try:
        db.execute("DELETE FROM auth_config")
        db.execute(
            "INSERT INTO auth_config "
            "(provider, azure_tenant_id, azure_client_id, "
            "google_client_id, google_client_secret, jwt_secret, "
            "updated_at, updated_by) "
            "VALUES (@param0, @param1, @param2, @param3, @param4, @param5, @param6, @param7)",
            [
                provider,
                azure_tenant_id,
                azure_client_id,
                google_client_id,
                enc_google_secret,
                enc_jwt_secret,
                now,
                updated_by,
            ],
        )
    except Exception as e:
        print(f"[AuthConfig] upsert_config failed: {e}")
        raise

    refresh_auth_config()
    return get_config()


def get_jwt_secret() -> str:
    """
    Return the decrypted jwt_secret from DB.
    On first call (no row / empty), generates a random secret, stores it, and returns it.
    Falls back to an HMAC-derived secret if DB is unavailable.
    """
    try:
        row = _get_raw_row()
        if row and row.get("jwt_secret"):
            try:
                return _decrypt(row["jwt_secret"])
            except Exception as e:
                print(f"[AuthConfig] jwt_secret decrypt failed — regenerating: {e}")

        # Generate and persist a new secret
        new_secret = secrets.token_hex(32)
        enc = _encrypt(new_secret)
        now = datetime.now(timezone.utc).replace(tzinfo=None)

        if row:
            # Row exists but jwt_secret is empty — update it
            db.execute(
                "UPDATE auth_config SET jwt_secret = @param0, updated_at = @param1",
                [enc, now],
            )
        else:
            # No row yet — insert a minimal local row
            db.execute(
                "INSERT INTO auth_config (provider, jwt_secret, updated_at, updated_by) "
                "VALUES ('local', @param0, @param1, 'system')",
                [enc, now],
            )
        return new_secret

    except Exception as e:
        print(f"[AuthConfig] get_jwt_secret DB error — using derived fallback: {e}")
        # Deterministic fallback from encryption secret
        secret = settings.AI_ENCRYPTION_SECRET or "loomx-default-jwt-fallback"
        return hashlib.sha256(f"jwt:{secret}".encode()).hexdigest()


# ── Internal helpers ──────────────────────────────────────────────────────────

def _get_raw_row() -> Optional[dict]:
    """Return the raw auth_config row (with encrypted values) or None."""
    try:
        return db.query_one(
            "SELECT TOP 1 id, provider, azure_tenant_id, azure_client_id, "
            "google_client_id, google_client_secret, jwt_secret "
            "FROM auth_config ORDER BY id DESC"
        )
    except Exception:
        return None
