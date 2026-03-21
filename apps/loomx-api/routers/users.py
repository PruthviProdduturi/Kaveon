"""Users router — /api/v1/users.

Endpoints:
  GET  /users/me              — current user's email + role
  GET  /users                 — all role assignments  (Admin only)
  PUT  /users/{email}/role    — assign/update a role   (Admin only)
  DELETE /users/{email}/role  — remove assignment      (Admin only)
"""

from fastapi import APIRouter, HTTPException, Depends
from middleware.auth import require_user_context, UserContext
from middleware.permissions import require_min_role
from models.users import RoleAssignBody
import services.users as users_svc

router = APIRouter()


@router.get("/users/me")
def get_me(ctx: UserContext = Depends(require_user_context)):
    return {
        "email": ctx.email,
        "role": ctx.role,
        "jwt_roles": ctx.jwt_roles,
    }


@router.get("/users")
def list_users(ctx: UserContext = Depends(require_min_role("Admin"))):
    return {"users": users_svc.list_role_assignments()}


@router.put("/users/{email}/role")
def assign_role(
    email: str,
    body: RoleAssignBody,
    ctx: UserContext = Depends(require_min_role("Admin")),
):
    # Admins cannot demote themselves
    if email.lower() == ctx.email and body.role != "Admin":
        raise HTTPException(
            status_code=400,
            detail={"code": "self_demotion", "message": "Admins cannot change their own role."},
        )
    record = users_svc.assign_role(email.lower(), body.role, ctx.email)
    return {"success": True, "assignment": record}


@router.delete("/users/{email}/role", status_code=204)
def remove_role(
    email: str,
    ctx: UserContext = Depends(require_min_role("Admin")),
):
    if email.lower() == ctx.email:
        raise HTTPException(
            status_code=400,
            detail={"code": "self_removal", "message": "Admins cannot remove their own role."},
        )
    users_svc.remove_role(email.lower())
