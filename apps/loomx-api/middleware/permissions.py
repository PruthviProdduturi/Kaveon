"""
Permission helpers for role-based access control.

Exports:
    ROLE_LEVELS          — dict mapping role name to numeric level
    require_min_role(r)  — FastAPI dependency factory; returns UserContext or raises 403
    can_read(...)        — check read access given visibility + owner
    can_write(...)       — check write (edit/delete) access
    can_publish(...)     — check if user may set visibility='published'
    can_admin(...)       — check if user has Admin role
"""

from fastapi import HTTPException, Depends

# Import here to avoid circular — auth defines UserContext
from middleware.auth import UserContext, get_user_context

ROLE_LEVELS: dict[str, int] = {
    "Viewer": 0,
    "Analyst": 1,
    "Editor": 2,
    "Admin": 3,
}


def _level(role: str) -> int:
    return ROLE_LEVELS.get(role, 0)


def require_min_role(min_role: str):
    """
    FastAPI dependency factory.

    Usage:
        @router.post("/something", dependencies=[Depends(require_min_role("Analyst"))])
        def endpoint(ctx: UserContext = Depends(require_min_role("Analyst"))): ...
    """
    def _dep(ctx: UserContext = Depends(get_user_context)) -> UserContext:
        if _level(ctx.role) < _level(min_role):
            raise HTTPException(
                status_code=403,
                detail={
                    "code": "forbidden",
                    "message": f"'{min_role}' role or higher is required.",
                    "your_role": ctx.role,
                },
            )
        return ctx
    return _dep


def can_read(visibility: str, owner_email: str, ctx: UserContext) -> bool:
    """Return True if ctx has read access to an item with this visibility and owner."""
    if ctx.role == "Admin":
        return True
    if visibility == "published":
        return True
    if visibility == "internal":
        return _level(ctx.role) >= _level("Analyst")
    if visibility == "private":
        return ctx.email == owner_email
    return False


def can_write(owner_email: str, ctx: UserContext) -> bool:
    """
    Return True if ctx may edit or delete an item owned by owner_email.
    Admins and Editors can touch anyone's content.
    Analysts can only touch their own.
    """
    if _level(ctx.role) >= _level("Editor"):
        return True
    return ctx.email == owner_email


def can_publish(ctx: UserContext) -> bool:
    """Return True if ctx may set visibility = 'published'."""
    return _level(ctx.role) >= _level("Editor")


def can_admin(ctx: UserContext) -> bool:
    """Return True if ctx is an Admin."""
    return ctx.role == "Admin"
