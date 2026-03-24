"""Users service — role resolution and management."""

from datetime import datetime, timezone
from typing import Optional
import database.metadata as db

ROLE_LEVELS: dict[str, int] = {
    "Viewer": 0,
    "Analyst": 1,
    "Editor": 2,
    "Admin": 3,
}

VALID_ROLES = set(ROLE_LEVELS.keys())


def resolve_role(user_email: str, jwt_roles: list[str], provider: str = "local") -> Optional[str]:
    """
    Determine the effective role for a user.

    For azure_ad / google: role comes exclusively from JWT claims (App Role
    assignments in Azure AD). No DB lookup, no bootstrap.

    For local auth: falls back to the user_roles DB table, then Viewer.

    Returns None to signal 403 NoAccess — authenticated but not assigned any role.
    """
    # 1. JWT roles — source of truth for oauth providers
    valid_jwt = [r for r in jwt_roles if r in VALID_ROLES]
    if valid_jwt:
        return max(valid_jwt, key=lambda r: ROLE_LEVELS[r])

    # For oauth providers JWT is the only source — no DB fallback, no bootstrap
    if provider in ("azure_ad", "google"):
        return None  # No App Role assigned — caller rejects with 403

    # 2. Local auth: DB assignment
    try:
        row = db.query_one(
            "SELECT role FROM user_roles WHERE user_email = @param0",
            [user_email],
        )
        if row and row.get("role"):
            return row["role"]
    except Exception as e:
        print(f"[Users] resolve_role DB lookup failed for {user_email}: {e}")

    # 3. Local default: Viewer
    return "Viewer"


def get_user_role(user_email: str) -> Optional[str]:
    """Return the DB-assigned role for a user, or None if not set."""
    try:
        row = db.query_one(
            "SELECT role FROM user_roles WHERE user_email = @param0",
            [user_email],
        )
        return row["role"] if row else None
    except Exception:
        return None


def list_role_assignments() -> list[dict]:
    """Return all explicit role assignments (Admin UI view)."""
    try:
        result = db.query(
            "SELECT user_email, role, granted_by, granted_at, updated_at "
            "FROM user_roles ORDER BY granted_at DESC",
        )
        return result["rows"]
    except Exception:
        return []


def assign_role(user_email: str, role: str, granted_by: str) -> dict:
    """Create or update a role assignment. Returns the saved record."""
    if role not in VALID_ROLES:
        raise ValueError(f"Invalid role '{role}'. Must be one of: {', '.join(VALID_ROLES)}")

    now = datetime.now(timezone.utc).replace(tzinfo=None)
    existing = db.query_one(
        "SELECT id FROM user_roles WHERE user_email = @param0",
        [user_email],
    )

    if existing:
        db.execute(
            "UPDATE user_roles SET role = @param0, granted_by = @param1, updated_at = @param2 "
            "WHERE user_email = @param3",
            [role, granted_by, now, user_email],
        )
    else:
        db.execute(
            "INSERT INTO user_roles (user_email, role, granted_by, granted_at, updated_at) "
            "VALUES (@param0, @param1, @param2, @param3, @param4)",
            [user_email, role, granted_by, now, now],
        )

    row = db.query_one(
        "SELECT user_email, role, granted_by, granted_at, updated_at "
        "FROM user_roles WHERE user_email = @param0",
        [user_email],
    )
    return row or {}


def remove_role(user_email: str) -> bool:
    """Remove an explicit role assignment, reverting the user to Viewer."""
    count = db.execute(
        "DELETE FROM user_roles WHERE user_email = @param0",
        [user_email],
    )
    return (count or 0) > 0
