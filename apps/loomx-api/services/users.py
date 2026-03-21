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


def resolve_role(user_email: str, jwt_roles: list[str]) -> str:
    """
    Determine the effective role for a user.

    Priority (highest wins):
      1. JWT roles claim  — assigned via Azure AD Enterprise Apps
      2. user_roles table — assigned via LoomX Admin UI
      3. Bootstrap        — if NO admins exist yet, first user becomes Admin
      4. Default          — Viewer

    This means an Azure AD assignment always wins over a LoomX UI assignment,
    and the LoomX UI wins over the default Viewer. Admin bootstrap only fires
    once on a fresh deployment.
    """
    try:
        # 1. Resolve JWT role (highest valid role in the claim)
        valid_jwt = [r for r in jwt_roles if r in VALID_ROLES]
        jwt_role = max(valid_jwt, key=lambda r: ROLE_LEVELS[r]) if valid_jwt else None

        # 2. Resolve DB role
        row = db.query_one(
            "SELECT role FROM user_roles WHERE user_email = @param0",
            [user_email],
        )
        db_role = row["role"] if row else None

        # Take highest of the two
        candidates = [r for r in [jwt_role, db_role] if r]
        if candidates:
            return max(candidates, key=lambda r: ROLE_LEVELS[r])

        # 3. Bootstrap: if no admins exist yet, this user becomes Admin
        count_row = db.query_one(
            "SELECT COUNT(*) AS n FROM user_roles WHERE role = 'Admin'",
        )
        if (count_row and (count_row.get("n") or 0) == 0):
            db.execute(
                "INSERT INTO user_roles (user_email, role, granted_by) "
                "VALUES (@param0, 'Admin', 'system-bootstrap')",
                [user_email],
            )
            return "Admin"

    except Exception as e:
        print(f"[Users] resolve_role failed for {user_email}: {e}")
        # DB unavailable — honour the cryptographically-signed JWT claim rather than
        # falling back to Viewer (which would silently strip Admin from local bootstrap users)
        if jwt_role:
            return jwt_role

    # 4. Default
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
