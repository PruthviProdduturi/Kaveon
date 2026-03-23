"""Auth config router.

Public endpoints (no auth required):
  GET  /auth/provider       — returns active provider + public OAuth params

Admin-only endpoints:
  GET    /v1/admin/auth                             — full config (secrets masked)
  POST   /v1/admin/auth                             — upsert config
  GET    /v1/admin/local-users                      — list local users
  POST   /v1/admin/local-users                      — create local user
  DELETE /v1/admin/local-users/{user_id}            — delete local user
  POST   /v1/admin/local-users/{user_id}/reset-password — reset password
"""

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel, Field
from typing import Optional

from middleware.auth import UserContext
from middleware.permissions import require_min_role
import services.auth_config as auth_config_svc
import services.local_auth as local_auth_svc

router = APIRouter()


# ── Pydantic models ───────────────────────────────────────────────────────────

class AuthConfigBody(BaseModel):
    provider: str = Field(..., pattern=r"^(azure_ad|local|google)$")
    azure_tenant_id: Optional[str] = None
    azure_client_id: Optional[str] = None
    google_client_id: Optional[str] = None
    google_client_secret: Optional[str] = None
    jwt_secret: Optional[str] = None




class CreateLocalUserBody(BaseModel):
    username: str = Field(..., min_length=1, max_length=255)
    email: str = Field(..., min_length=3, max_length=255)
    password: str = Field(..., min_length=6, max_length=128)
    role: str = Field(default="Viewer", pattern=r"^(Viewer|Analyst|Editor|Admin)$")


class ResetPasswordBody(BaseModel):
    new_password: str = Field(..., min_length=6, max_length=128)


# ── Public endpoint ───────────────────────────────────────────────────────────

@router.get("/auth/provider")
def get_provider():
    """
    Returns the active auth provider and any public OAuth parameters.
    No authentication required — called by the frontend login page.
    """
    cfg = auth_config_svc.get_config()
    provider = cfg.get("provider", "local")

    response: dict = {"provider": provider}

    if provider == "azure_ad":
        if cfg.get("azure_client_id"):
            response["azure_client_id"] = cfg["azure_client_id"]
        if cfg.get("azure_tenant_id"):
            response["azure_tenant_id"] = cfg["azure_tenant_id"]
    elif provider == "google":
        if cfg.get("google_client_id"):
            response["google_client_id"] = cfg["google_client_id"]

    return response


# ── Admin — auth config ───────────────────────────────────────────────────────

@router.get("/v1/admin/auth")
def admin_get_auth_config(ctx: UserContext = Depends(require_min_role("Admin"))):
    """Return the current auth configuration (secrets masked)."""
    return auth_config_svc.get_config()


@router.post("/v1/admin/auth")
def admin_upsert_auth_config(
    body: AuthConfigBody,
    ctx: UserContext = Depends(require_min_role("Admin")),
):
    """Upsert the auth configuration. Triggers a middleware cache refresh."""
    data = body.model_dump()
    data["updated_by"] = ctx.email
    saved = auth_config_svc.upsert_config(data)
    auth_config_svc.refresh_auth_config()
    return {"success": True, "config": saved}


class SetupAppRolesBody(BaseModel):
    graph_token: str = Field(..., min_length=1)
    client_id:   str = Field(..., min_length=1)
    tenant_id:   str = Field(..., min_length=1)


@router.post("/v1/admin/auth/setup-app-roles")
def admin_setup_app_roles(
    body: SetupAppRolesBody,
    ctx: UserContext = Depends(require_min_role("Admin")),
):
    """
    Create the 4 LoomX App Roles in the Azure AD App Registration using a
    delegated Graph API token acquired by the frontend via MSAL popup.
    Idempotent — roles that already exist are skipped.
    Returns 403 with a clear message if the user lacks owner/write permission.
    """
    try:
        result = auth_config_svc.setup_azure_app_roles(
            graph_token=body.graph_token,
            client_id=body.client_id,
            tenant_id=body.tenant_id,
        )
        return result
    except PermissionError as e:
        raise HTTPException(
            status_code=403,
            detail={"code": "insufficient_permissions", "message": str(e)},
        )
    except ValueError as e:
        raise HTTPException(
            status_code=400,
            detail={"code": "setup_failed", "message": str(e)},
        )
    except Exception as e:
        raise HTTPException(
            status_code=500,
            detail={"code": "unexpected_error", "message": str(e)},
        )


# ── Admin — local users ───────────────────────────────────────────────────────

@router.get("/v1/admin/local-users")
def admin_list_local_users(ctx: UserContext = Depends(require_min_role("Admin"))):
    """List all local users."""
    return {"users": local_auth_svc.list_local_users()}


@router.post("/v1/admin/local-users", status_code=201)
def admin_create_local_user(
    body: CreateLocalUserBody,
    ctx: UserContext = Depends(require_min_role("Admin")),
):
    """Create a new local user."""
    try:
        user = local_auth_svc.create_local_user(
            username=body.username,
            email=body.email,
            password=body.password,
            role=body.role,
        )
        return {"success": True, "user": user}
    except Exception as e:
        raise HTTPException(
            status_code=400,
            detail={"code": "create_user_failed", "message": str(e)},
        )


@router.delete("/v1/admin/local-users/{user_id}", status_code=204)
def admin_delete_local_user(
    user_id: int,
    ctx: UserContext = Depends(require_min_role("Admin")),
):
    """Delete a local user by id."""
    deleted = local_auth_svc.delete_local_user(user_id)
    if not deleted:
        raise HTTPException(
            status_code=404,
            detail={"code": "not_found", "message": f"Local user {user_id} not found."},
        )


@router.post("/v1/admin/local-users/{user_id}/reset-password")
def admin_reset_password(
    user_id: int,
    body: ResetPasswordBody,
    ctx: UserContext = Depends(require_min_role("Admin")),
):
    """Reset a local user's password."""
    ok = local_auth_svc.update_password(user_id, body.new_password)
    if not ok:
        raise HTTPException(
            status_code=404,
            detail={"code": "not_found", "message": f"Local user {user_id} not found."},
        )
    return {"success": True}
