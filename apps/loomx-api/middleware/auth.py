"""
Authentication middleware.

Security model:
  - When AZURE_CLIENT_ID + AZURE_TENANT_ID are set (production):
      ONLY a valid signed Bearer JWT grants authentication.
      The x-user-email header is IGNORED.
  - When Azure AD is NOT configured (first-run setup mode):
      Fall back to unverified JWT decode, then x-user-email header.

Dependencies exported:
    get_current_user(request)   → str | None          (email only, backward compat)
    require_auth(user)          → str                  (401 if None)
    get_user_email(request)     → str                  (helper used across routers)
    UserContext                 — dataclass(email, role, jwt_roles)
    get_user_context(request)   → UserContext | None   (email + resolved role)
    require_user_context(ctx)   → UserContext          (401 if None)
"""

from dataclasses import dataclass, field
from typing import Optional

import jwt
from jwt import PyJWKClient, ExpiredSignatureError, InvalidTokenError
from fastapi import Request, HTTPException, Depends

from config import settings

# ── JWKS client (Azure AD public keys) ────────────────────────────────────────
_TENANT_ID = settings.AZURE_TENANT_ID or "common"
_JWKS_URL = f"https://login.microsoftonline.com/{_TENANT_ID}/discovery/v2.0/keys"

_AAD_CONFIGURED = bool(settings.AZURE_CLIENT_ID and settings.AZURE_TENANT_ID)

_jwks_client: Optional[PyJWKClient] = None
if _AAD_CONFIGURED:
    _jwks_client = PyJWKClient(_JWKS_URL, cache_keys=True)


# ── UserContext ────────────────────────────────────────────────────────────────

@dataclass
class UserContext:
    """Authenticated user with resolved role. Passed through role-aware endpoints."""
    email: str
    role: str                        # 'Viewer' | 'Analyst' | 'Editor' | 'Admin'
    jwt_roles: list[str] = field(default_factory=list)


# ── JWT helpers ───────────────────────────────────────────────────────────────

def _email_from_payload(payload: dict) -> Optional[str]:
    email = (
        payload.get("preferred_username")
        or payload.get("email")
        or payload.get("upn")
    )
    if isinstance(email, str) and "@" in email:
        return email.lower()
    return None


def _roles_from_payload(payload: dict) -> list[str]:
    """Extract Azure AD App Role claims from the JWT payload."""
    VALID = {"Viewer", "Analyst", "Editor", "Admin"}
    raw = payload.get("roles", [])
    if not isinstance(raw, list):
        return []
    return [r for r in raw if r in VALID]


def _decode_token(token: str) -> Optional[tuple[str, list[str]]]:
    """
    Decode and verify a Bearer JWT.
    Returns (email, jwt_roles) or None if invalid/expired.
    """
    if not token:
        return None

    if _jwks_client is None:
        # Setup mode — AAD not configured.
        try:
            payload = jwt.decode(token, options={"verify_signature": False})
            email = _email_from_payload(payload)
            return (email, []) if email else None
        except Exception:
            return None

    _valid_audiences = [
        settings.AZURE_CLIENT_ID,
        f"api://{settings.AZURE_CLIENT_ID}",
    ]
    try:
        signing_key = _jwks_client.get_signing_key_from_jwt(token)
        payload = jwt.decode(
            token,
            signing_key.key,
            algorithms=["RS256"],
            audience=_valid_audiences,
            options={"verify_exp": True},
        )
        email = _email_from_payload(payload)
        if not email:
            return None
        roles = _roles_from_payload(payload)
        return (email, roles)
    except (ExpiredSignatureError, InvalidTokenError):
        return None
    except Exception:
        return None


# ── FastAPI dependencies (email-only, backward-compatible) ────────────────────

def get_current_user(request: Request) -> Optional[str]:
    """
    Non-blocking dependency — returns user email or None.
    Preserved for backward compatibility with routers that only need the email.
    """
    auth_header = request.headers.get("authorization", "")
    if auth_header.startswith("Bearer "):
        token = auth_header[7:].strip()
        result = _decode_token(token)
        if result:
            email, _ = result
            request.state.user = email
            return email

    if not _AAD_CONFIGURED:
        email = request.headers.get("x-user-email", "")
        if email and "@" in email:
            request.state.user = email.lower()
            return email.lower()

    request.state.user = None
    return None


def require_auth(user: Optional[str] = Depends(get_current_user)) -> str:
    """Returns email or raises 401. Backward-compatible with existing routers."""
    if not user:
        raise HTTPException(
            status_code=401,
            detail={"code": "unauthorized", "message": "Authentication required."},
        )
    return user


def get_user_email(request: Request) -> str:
    user = getattr(request.state, "user", None)
    if not user:
        user = get_current_user(request)
    return user or "anonymous"


# ── Role-aware dependencies ───────────────────────────────────────────────────

def get_user_context(request: Request) -> Optional[UserContext]:
    """
    Returns a UserContext with email + resolved role, or None if unauthenticated.
    Resolves role from: JWT claim → user_roles table → bootstrap → Viewer default.
    """
    auth_header = request.headers.get("authorization", "")
    email: Optional[str] = None
    jwt_roles: list[str] = []

    if auth_header.startswith("Bearer "):
        token = auth_header[7:].strip()
        result = _decode_token(token)
        if result:
            email, jwt_roles = result

    if not email and not _AAD_CONFIGURED:
        raw = request.headers.get("x-user-email", "")
        if raw and "@" in raw:
            email = raw.lower()

    if not email:
        return None

    request.state.user = email

    # Resolve role (JWT → DB → bootstrap → Viewer)
    # Import here to avoid module-level circular dependency
    try:
        import services.users as users_svc
        role = users_svc.resolve_role(email, jwt_roles)
    except Exception as e:
        print(f"[Auth] Role resolution failed for {email}: {e}")
        # In setup mode or DB unavailable, grant Admin so setup can proceed
        role = "Admin" if not _AAD_CONFIGURED else "Viewer"

    ctx = UserContext(email=email, role=role, jwt_roles=jwt_roles)
    request.state.user_context = ctx
    return ctx


def require_user_context(ctx: Optional[UserContext] = Depends(get_user_context)) -> UserContext:
    """Returns UserContext or raises 401."""
    if not ctx:
        raise HTTPException(
            status_code=401,
            detail={"code": "unauthorized", "message": "Authentication required."},
        )
    return ctx
