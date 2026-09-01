"""Users router — /api/v1/users.

Endpoints:
  GET  /users/me  — current user's email + role
"""

from fastapi import APIRouter, HTTPException, Depends
from middleware.auth import require_user_context, UserContext

router = APIRouter()


@router.get("/users/me")
def get_me(ctx: UserContext = Depends(require_user_context)):
    if ctx.role == "NoAccess":
        raise HTTPException(
            status_code=403,
            detail={
                "code": "no_role",
                "message": "You have not been assigned a role in Kaveon. Contact your administrator to be assigned a role.",
            },
        )
    return {
        "email": ctx.email,
        "role": ctx.role,
        "jwt_roles": ctx.jwt_roles,
    }
