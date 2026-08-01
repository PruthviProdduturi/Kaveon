"""Users service — role resolution."""

from typing import Optional

ROLE_LEVELS: dict[str, int] = {
    "Viewer": 0,
    "Analyst": 1,
    "Editor": 2,
    "Admin": 3,
}

VALID_ROLES = set(ROLE_LEVELS.keys())


def resolve_role(user_email: str, jwt_roles: list[str], provider: str = "local") -> Optional[str]:
    """
    Determine the effective role for a user from JWT claims.

    For azure_ad / google: role comes exclusively from App Role assignments
    embedded in the JWT. Returns None if no role → caller rejects with 403.

    For local auth: role is embedded in the JWT at login time from
    local_users.role. Falls back to "Viewer" if the claim is absent.
    """
    valid_jwt = [r for r in jwt_roles if r in VALID_ROLES]
    if valid_jwt:
        return max(valid_jwt, key=lambda r: ROLE_LEVELS[r])

    if provider in ("azure_ad", "google"):
        return None  # No App Role assigned — caller rejects with 403

    return "Viewer"  # Local auth fallback
