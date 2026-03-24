"""Local auth service — bcrypt password management and local user CRUD."""

from datetime import datetime, timezone
from typing import Optional

import bcrypt

import database.metadata as db

# ── Bootstrap hash — bcrypt("admin", cost=12) pre-computed at import time ────
# Used as a fallback when the DB is not yet reachable on the very first boot.

_BOOTSTRAP_HASH: bytes = bcrypt.hashpw(b"admin", bcrypt.gensalt(rounds=12))


# ── Password helpers ──────────────────────────────────────────────────────────

def hash_password(plain: str) -> str:
    """Hash *plain* with bcrypt (cost 12). Returns the hash as a str."""
    return bcrypt.hashpw(plain.encode(), bcrypt.gensalt(rounds=12)).decode()


def verify_password(plain: str, hashed: str) -> bool:
    """Return True if *plain* matches *hashed*."""
    try:
        return bcrypt.checkpw(plain.encode(), hashed.encode())
    except Exception:
        return False


# ── User queries ──────────────────────────────────────────────────────────────

def get_user_by_username(username: str) -> Optional[dict]:
    """Return a local_users row by username, or None.
    Raises on DB errors so the caller can detect DB unavailability."""
    return db.query_one(
        "SELECT id, username, email, password_hash, role, force_password_change, is_active "
        "FROM local_users WHERE username = @param0",
        [username],
    )


def get_user_by_id(user_id: int) -> Optional[dict]:
    """Return a local_users row by id, or None."""
    try:
        return db.query_one(
            "SELECT id, username, email, password_hash, role, force_password_change, is_active "
            "FROM local_users WHERE id = @param0",
            [user_id],
        )
    except Exception as e:
        print(f"[LocalAuth] get_user_by_id failed: {e}")
        return None


# ── Bootstrap ─────────────────────────────────────────────────────────────────

def bootstrap_admin_if_needed() -> None:
    """
    Ensure the default admin/admin@local user exists in local_users with Admin role.
    Called at every API startup. Creates the user if local_users is empty.
    """
    try:
        row = db.query_one("SELECT COUNT(*) AS n FROM local_users")
        count = int(row.get("n", 0)) if row else 0
        if count == 0:
            now = datetime.now(timezone.utc).replace(tzinfo=None)
            pwd_hash = hash_password("admin")
            db.execute(
                "INSERT INTO local_users "
                "(username, email, password_hash, role, force_password_change, is_active, created_at, updated_at) "
                "VALUES (@param0, @param1, @param2, @param3, @param4, @param5, @param6, @param7)",
                ["admin", "admin@local", pwd_hash, "Admin", True, True, now, now],
            )
            print("[LocalAuth] Bootstrap admin user created (admin / admin@local).")
    except Exception as e:
        print(f"[LocalAuth] bootstrap_admin_if_needed failed: {e}")


# ── CRUD ──────────────────────────────────────────────────────────────────────

def create_local_user(username: str, email: str, password: str, role: str) -> dict:
    """Create a new local user. Role is stored in local_users.role."""
    now = datetime.now(timezone.utc).replace(tzinfo=None)
    pwd_hash = hash_password(password)

    db.execute(
        "INSERT INTO local_users "
        "(username, email, password_hash, role, force_password_change, is_active, created_at, updated_at) "
        "VALUES (@param0, @param1, @param2, @param3, @param4, @param5, @param6, @param7)",
        [username, email.lower(), pwd_hash, role, False, True, now, now],
    )

    row = db.query_one(
        "SELECT id, username, email, role, force_password_change, is_active, created_at "
        "FROM local_users WHERE username = @param0",
        [username],
    )
    return row or {}


def update_password(user_id: int, new_password: str) -> bool:
    """Update the password for a local user. Returns True on success."""
    try:
        now = datetime.now(timezone.utc).replace(tzinfo=None)
        pwd_hash = hash_password(new_password)
        affected = db.execute(
            "UPDATE local_users "
            "SET password_hash = @param0, force_password_change = @param1, updated_at = @param2 "
            "WHERE id = @param3",
            [pwd_hash, False, now, user_id],
        )
        return (affected or 0) > 0
    except Exception as e:
        print(f"[LocalAuth] update_password failed: {e}")
        return False


def delete_local_user(user_id: int) -> bool:
    """Delete a local user. Returns True if a row was removed."""
    try:
        affected = db.execute(
            "DELETE FROM local_users WHERE id = @param0",
            [user_id],
        )
        return (affected or 0) > 0
    except Exception as e:
        print(f"[LocalAuth] delete_local_user failed: {e}")
        return False


def list_local_users() -> list[dict]:
    """Return all local users (no password_hash)."""
    try:
        result = db.query(
            "SELECT id, username, email, force_password_change, is_active, created_at, updated_at "
            "FROM local_users ORDER BY id"
        )
        return result["rows"]
    except Exception as e:
        print(f"[LocalAuth] list_local_users failed: {e}")
        return []
