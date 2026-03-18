"""
Authentication middleware.

Port of authMiddleware.ts — with JWT *signature* verification added.

Security model:
  - When AZURE_CLIENT_ID + AZURE_TENANT_ID are set (production):
      ONLY a valid signed Bearer JWT grants authentication.
      The x-user-email header is IGNORED — it can be spoofed by any caller.
  - When Azure AD is NOT configured (first-run setup mode):
      Fall back to unverified JWT decode, then x-user-email header.
      This is intentional — no metadata DB exists yet to verify against.

Dependencies exported:
    get_current_user(request)  → str | None  (FastAPI dependency, non-blocking)
    require_auth(user)         → str          (FastAPI dependency, returns 401 if None)
    get_user_email(request)    → str          (helper used across routers)
"""

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


def _email_from_payload(payload: dict) -> Optional[str]:
    """Extract email from Azure AD JWT claims (preferred_username / email / upn)."""
    email = (
        payload.get("preferred_username")
        or payload.get("email")
        or payload.get("upn")
    )
    if isinstance(email, str) and "@" in email:
        return email.lower()
    return None


def _extract_email_from_token(token: str) -> Optional[str]:
    """
    Decode and verify a Bearer JWT.
    Returns the user's email, or None if the token is invalid/expired.
    """
    if not token:
        return None

    if _jwks_client is None:
        # Setup mode — AAD not yet configured. Accept unverified decode only
        # so the setup wizard can operate. Never reached in production.
        try:
            payload = jwt.decode(token, options={"verify_signature": False})
            return _email_from_payload(payload)
        except Exception:
            return None

    try:
        signing_key = _jwks_client.get_signing_key_from_jwt(token)
        payload = jwt.decode(
            token,
            signing_key.key,
            algorithms=["RS256"],
            audience=settings.AZURE_CLIENT_ID,
            options={"verify_exp": True},
        )
        return _email_from_payload(payload)
    except (ExpiredSignatureError, InvalidTokenError):
        return None
    except Exception:
        return None


# ── FastAPI dependencies ──────────────────────────────────────────────────────

def get_current_user(request: Request) -> Optional[str]:
    """
    Non-blocking dependency — attaches the authenticated user's email to
    request.state.user.

    When AAD is configured (production): ONLY a valid signed Bearer JWT works.
    When AAD is not configured (setup mode): also accepts x-user-email header.
    """
    auth_header = request.headers.get("authorization", "")
    if auth_header.startswith("Bearer "):
        token = auth_header[7:].strip()
        user = _extract_email_from_token(token)
        if user:
            request.state.user = user
            return user

    # In setup mode (no AAD config) only: accept x-user-email as fallback
    if not _AAD_CONFIGURED:
        email = request.headers.get("x-user-email", "")
        if email and "@" in email:
            request.state.user = email.lower()
            return email.lower()

    request.state.user = None
    return None


def require_auth(user: Optional[str] = Depends(get_current_user)) -> str:
    """Dependency that returns 401 if no authenticated user is present."""
    if not user:
        raise HTTPException(
            status_code=401,
            detail={"code": "unauthorized", "message": "Authentication required. Include a valid Bearer token."},
        )
    return user


def get_user_email(request: Request) -> str:
    """
    Return the verified user email from JWT state, or 'anonymous'.
    Never trusts raw headers when AAD is configured.
    """
    user = getattr(request.state, "user", None)
    if not user:
        # Ensure get_current_user runs if middleware hasn't populated state yet
        user = get_current_user(request)
    return user or "anonymous"
