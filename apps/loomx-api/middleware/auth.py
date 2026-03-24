"""
Authentication middleware.

Security model:
  Provider is read from the auth_config table (or defaults to 'local' on first run).

  - azure_ad  → RS256 JWKS verification against Microsoft login endpoint.
  - local     → HS256 verification using the jwt_secret stored in auth_config.
  - google    → RS256 JWKS verification against Google's public certs endpoint.

  When no DB is available (first-run / setup mode):
      Falls back to unverified JWT decode, then x-user-email header.

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

# ── Azure AD JWKS client (kept for backward compat when provider=azure_ad) ───

_TENANT_ID = settings.AZURE_TENANT_ID or "common"
_AAD_JWKS_URL = f"https://login.microsoftonline.com/{_TENANT_ID}/discovery/v2.0/keys"
_GOOGLE_JWKS_URL = "https://www.googleapis.com/oauth2/v3/certs"

_AAD_CONFIGURED = bool(settings.AZURE_CLIENT_ID and settings.AZURE_TENANT_ID)

_aad_jwks_client: Optional[PyJWKClient] = None
if _AAD_CONFIGURED:
    _aad_jwks_client = PyJWKClient(_AAD_JWKS_URL, cache_keys=True)

# Google JWKS client — created lazily when first needed
_google_jwks_client: Optional[PyJWKClient] = None


def _get_google_jwks_client() -> PyJWKClient:
    global _google_jwks_client
    if _google_jwks_client is None:
        _google_jwks_client = PyJWKClient(_GOOGLE_JWKS_URL, cache_keys=True)
    return _google_jwks_client


# ── Provider resolution ───────────────────────────────────────────────────────

def _get_active_provider() -> str:
    """
    Return the active auth provider name.
    Calls services.auth_config.get_active_provider() — that module caches
    the value and can be cleared via refresh_auth_config().
    Falls back to 'azure_ad' if Azure AD env vars are set (legacy path),
    or 'local' if nothing is configured.
    """
    try:
        import services.auth_config as _ac
        return _ac.get_active_provider()
    except Exception:
        pass

    # Env-var legacy fallback
    if _AAD_CONFIGURED:
        return "azure_ad"
    return "local"


# ── UserContext ───────────────────────────────────────────────────────────────

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
    """Extract App Role claims from the JWT payload."""
    VALID = {"Viewer", "Analyst", "Editor", "Admin"}
    raw = payload.get("roles", [])
    if not isinstance(raw, list):
        return []
    return [r for r in raw if r in VALID]


def _decode_azure_ad(token: str) -> Optional[tuple[str, list[str]]]:
    """Verify an Azure AD RS256 token. Returns (email, roles) or None."""
    if _aad_jwks_client is None:
        return None
    _valid_audiences = [
        settings.AZURE_CLIENT_ID,
        f"api://{settings.AZURE_CLIENT_ID}",
    ]
    try:
        signing_key = _aad_jwks_client.get_signing_key_from_jwt(token)
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
        return (email, _roles_from_payload(payload))
    except (ExpiredSignatureError, InvalidTokenError):
        return None
    except Exception:
        return None


def _decode_local_hs256(token: str) -> Optional[tuple[str, list[str]]]:
    """Verify a local HS256 token. Returns (email, roles) or None."""
    try:
        import services.auth_config as _ac
        secret = _ac.get_jwt_secret()
        payload = jwt.decode(
            token,
            secret,
            algorithms=["HS256"],
            options={"verify_aud": False},
        )
        email = _email_from_payload(payload)
        if not email:
            return None
        return (email, _roles_from_payload(payload))
    except (ExpiredSignatureError, InvalidTokenError):
        return None
    except Exception:
        return None


def _decode_google(token: str) -> Optional[tuple[str, list[str]]]:
    """Verify a Google RS256 token. Returns (email, roles) or None."""
    try:
        import services.auth_config as _ac
        cfg = _ac.get_config()
        google_client_id = cfg.get("google_client_id") or ""
        if not google_client_id:
            return None

        client = _get_google_jwks_client()
        signing_key = client.get_signing_key_from_jwt(token)
        payload = jwt.decode(
            token,
            signing_key.key,
            algorithms=["RS256"],
            audience=google_client_id,
            options={"verify_exp": True},
        )
        # Google JWTs use 'email' claim
        email_raw = payload.get("email") or payload.get("preferred_username") or ""
        if not isinstance(email_raw, str) or "@" not in email_raw:
            return None
        email = email_raw.lower()
        return (email, _roles_from_payload(payload))
    except (ExpiredSignatureError, InvalidTokenError):
        return None
    except Exception:
        return None


def _decode_token(token: str) -> Optional[tuple[str, list[str]]]:
    """
    Decode and verify a Bearer JWT using the active provider.
    Returns (email, jwt_roles) or None if invalid/expired.
    """
    if not token:
        return None

    provider = _get_active_provider()

    if provider == "azure_ad":
        if _aad_jwks_client is not None:
            return _decode_azure_ad(token)
        # AAD selected but JWKS client unavailable — fall through to setup-mode decode
    elif provider == "local":
        result = _decode_local_hs256(token)
        if result:
            return result
        # If local secret not available yet, fall through
    elif provider == "google":
        return _decode_google(token)

    # Setup/fallback mode — accept any structurally valid JWT without signature verification.
    # Preserve the roles claim from the payload so bootstrap Admin tokens stay Admin.
    try:
        payload = jwt.decode(token, options={"verify_signature": False})
        email = _email_from_payload(payload)
        if not email:
            return None
        return (email, _roles_from_payload(payload))
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

    # x-user-email header fallback — only in setup mode (no AAD config and no DB provider)
    provider = _get_active_provider()
    if not _AAD_CONFIGURED and provider in ("local",):
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

    # x-user-email fallback — only when AAD is not configured and provider is local
    provider = _get_active_provider()
    if not email and not _AAD_CONFIGURED and provider in ("local",):
        raw = request.headers.get("x-user-email", "")
        if raw and "@" in raw:
            email = raw.lower()

    if not email:
        return None

    request.state.user = email

    # Resolve role (JWT → DB → bootstrap → Viewer/NoAccess)
    provider = _get_active_provider()
    try:
        import services.users as users_svc
        role = users_svc.resolve_role(email, jwt_roles, provider)
    except Exception as e:
        print(f"[Auth] Role resolution failed for {email}: {e}")
        role = "Admin" if not _AAD_CONFIGURED else None

    # None means the user is authenticated but has no role assigned
    ctx = UserContext(email=email, role=role or "NoAccess", jwt_roles=jwt_roles)
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
