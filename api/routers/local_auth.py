"""Local auth router.

Endpoints (no auth required):
  POST /auth/login          — username + password → HS256 JWT

Endpoints (Bearer JWT required):
  POST /auth/change-password — change own password
"""

from datetime import datetime, timedelta, timezone

import jwt
from fastapi import APIRouter, Depends, HTTPException, Request
from pydantic import BaseModel, Field

from middleware.auth import require_auth
import services.auth_config as auth_config_svc
import services.local_auth as local_auth_svc

router = APIRouter()

_TOKEN_TTL_HOURS = 8


# ── Pydantic models ───────────────────────────────────────────────────────────

class LoginBody(BaseModel):
    username: str = Field(..., min_length=1, max_length=255)
    password: str = Field(..., min_length=1, max_length=128)


class ChangePasswordBody(BaseModel):
    current_password: str = Field(..., min_length=1, max_length=128)
    new_password: str = Field(..., min_length=6, max_length=128)


# ── JWT helpers ───────────────────────────────────────────────────────────────

def _issue_jwt(email: str, name: str, roles: list[str]) -> str:
    """Issue an HS256 JWT valid for _TOKEN_TTL_HOURS hours."""
    secret = auth_config_svc.get_jwt_secret()
    now = datetime.now(timezone.utc)
    payload = {
        "sub": email,
        "preferred_username": email,
        "name": name,
        "roles": roles,
        "iss": "kaveon",
        "iat": int(now.timestamp()),
        "exp": int((now + timedelta(hours=_TOKEN_TTL_HOURS)).timestamp()),
    }
    return jwt.encode(payload, secret, algorithm="HS256")


def _resolve_roles_for_email(email: str) -> list[str]:
    """Return the role stored in local_users.role for this email."""
    try:
        import database.metadata as _db
        row = _db.query_one(
            "SELECT role FROM local_users WHERE email = @param0",
            [email],
        )
        if row and row.get("role"):
            return [row["role"]]
        return []
    except Exception as e:
        print(f"[LocalAuth] _resolve_roles_for_email failed for {email}: {e}")
        # DB unavailable (pre-migration) — grant Admin so first user can reach Settings
        return ["Admin"]


# ── Endpoints ─────────────────────────────────────────────────────────────────

@router.post("/auth/login")
def login(body: LoginBody):
    """
    Authenticate with username + password.
    Returns a signed HS256 JWT on success.

    Pre-DB fallback: if the DB is unreachable, falls back to the in-memory
    bootstrap admin check (username=="admin", password=="admin").
    """
    username = body.username.strip()
    password = body.password

    # Attempt DB-backed verification
    db_available = True
    user_row = None
    try:
        user_row = local_auth_svc.get_user_by_username(username)
    except Exception as e:
        from fastapi import HTTPException
        if not (isinstance(e, HTTPException) and e.status_code == 503):
            print(f"[LocalAuth] login DB error: {e}")
        db_available = False

    if db_available:
        if not user_row:
            raise HTTPException(
                status_code=401,
                detail={"code": "invalid_credentials", "message": "Invalid username or password."},
            )
        if not user_row.get("is_active"):
            raise HTTPException(
                status_code=403,
                detail={"code": "account_disabled", "message": "Account is disabled."},
            )
        if not local_auth_svc.verify_password(password, user_row["password_hash"]):
            raise HTTPException(
                status_code=401,
                detail={"code": "invalid_credentials", "message": "Invalid username or password."},
            )

        email = user_row["email"]
        name = user_row["username"]
        roles = _resolve_roles_for_email(email)
        token = _issue_jwt(email, name, roles)
        return {
            "access_token": token,
            "token_type": "Bearer",
            "force_password_change": bool(user_row.get("force_password_change")),
        }

    # Pre-DB fallback — only the bootstrap admin/admin is accepted
    import bcrypt as _bcrypt
    if username == "admin" and _bcrypt.checkpw(password.encode(), local_auth_svc._BOOTSTRAP_HASH):
        token = _issue_jwt("admin@local", "admin", ["Admin"])
        return {
            "access_token": token,
            "token_type": "Bearer",
            "force_password_change": True,
        }

    raise HTTPException(
        status_code=401,
        detail={"code": "invalid_credentials", "message": "Invalid username or password."},
    )


@router.post("/auth/change-password")
def change_password(body: ChangePasswordBody, user_email: str = Depends(require_auth)):
    """
    Change own password.  Requires a valid Bearer JWT.
    """
    try:
        import database.metadata as db
        user_row = db.query_one(
            "SELECT id, password_hash FROM local_users WHERE email = @param0",
            [user_email],
        )
    except Exception as e:
        raise HTTPException(
            status_code=503,
            detail={"code": "db_unavailable", "message": "Database is unavailable."},
        )

    if not user_row:
        raise HTTPException(
            status_code=404,
            detail={"code": "not_found", "message": "Local user account not found."},
        )

    if not local_auth_svc.verify_password(body.current_password, user_row["password_hash"]):
        raise HTTPException(
            status_code=401,
            detail={"code": "invalid_credentials", "message": "Current password is incorrect."},
        )

    ok = local_auth_svc.update_password(user_row["id"], body.new_password)
    if not ok:
        raise HTTPException(
            status_code=500,
            detail={"code": "update_failed", "message": "Password update failed."},
        )

    return {"success": True, "message": "Password updated successfully."}
