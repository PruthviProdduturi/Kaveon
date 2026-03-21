"""AI router — /api/v1/ai.

Endpoints:
  GET    /ai/providers          — list active providers (Admin: full detail)
  POST   /ai/providers          — add/update a global provider key  (Admin)
  DELETE /ai/providers/{id}     — remove a global provider          (Admin)
  GET    /ai/my-keys            — current user's personal keys      (Analyst+)
  PUT    /ai/my-keys            — save/update personal key          (Analyst+)
  DELETE /ai/my-keys/{provider} — delete personal key               (Analyst+)
  POST   /ai/chat               — send a message to the AI          (Analyst+)
"""

import time
import threading
from fastapi import APIRouter, HTTPException, Depends

from middleware.auth import UserContext
from middleware.permissions import require_min_role
from models.ai import AIProviderBody, UserAIKeyBody, ChatRequest
import services.ai_service as ai_svc

router = APIRouter()

# ── Rate limiter (30 AI calls per user per minute) ───────────────────────────
_AI_RATE: dict[str, list[float]] = {}
_AI_RATE_LOCK = threading.Lock()
_AI_RATE_LIMIT = 30
_AI_RATE_WINDOW = 60


def _check_rate(user_email: str) -> None:
    now = time.time()
    with _AI_RATE_LOCK:
        timestamps = [t for t in _AI_RATE.get(user_email, []) if now - t < _AI_RATE_WINDOW]
        if len(timestamps) >= _AI_RATE_LIMIT:
            raise HTTPException(status_code=429, detail={"code": "rate_limit", "message": "Too many AI requests. Please wait a moment."})
        timestamps.append(now)
        _AI_RATE[user_email] = timestamps


# ── Table bootstrap on first import ──────────────────────────────────────────
try:
    ai_svc.ensure_tables()
except Exception as e:
    print(f"[AI router] Table bootstrap warning: {e}")


# ── Provider management (Admin) ───────────────────────────────────────────────

@router.get("/ai/providers")
def list_providers(ctx: UserContext = Depends(require_min_role("Analyst"))):
    rows = ai_svc.list_providers()
    if ctx.role != "Admin":
        # Non-admins only see which providers are available (no keys)
        return {"providers": [{"id": r["id"], "provider": r["provider"], "label": r["label"], "model": r["model"], "is_active": r["is_active"]} for r in rows]}
    return {"providers": rows}


@router.post("/ai/providers", status_code=201)
def add_provider(body: AIProviderBody, ctx: UserContext = Depends(require_min_role("Admin"))):
    if body.provider not in ("anthropic", "openai"):
        raise HTTPException(status_code=400, detail={"code": "invalid_provider", "message": "Provider must be 'anthropic' or 'openai'."})
    result = ai_svc.upsert_provider(
        provider=body.provider,
        label=body.label,
        api_key=body.api_key,
        model=body.model,
        is_active=body.is_active,
        created_by=ctx.email,
    )
    return {"success": True, "provider": result}


@router.delete("/ai/providers/{provider_id}", status_code=204)
def delete_provider(provider_id: int, ctx: UserContext = Depends(require_min_role("Admin"))):
    ai_svc.delete_provider(provider_id)


# ── User personal keys ────────────────────────────────────────────────────────

@router.get("/ai/my-keys")
def get_my_keys(ctx: UserContext = Depends(require_min_role("Analyst"))):
    return {"keys": ai_svc.get_user_keys(ctx.email)}


@router.put("/ai/my-keys")
def save_my_key(body: UserAIKeyBody, ctx: UserContext = Depends(require_min_role("Analyst"))):
    if body.provider not in ("anthropic", "openai"):
        raise HTTPException(status_code=400, detail={"code": "invalid_provider", "message": "Provider must be 'anthropic' or 'openai'."})
    ai_svc.upsert_user_key(ctx.email, body.provider, body.api_key, body.model)
    return {"success": True}


@router.delete("/ai/my-keys/{provider}", status_code=204)
def delete_my_key(provider: str, ctx: UserContext = Depends(require_min_role("Analyst"))):
    ai_svc.delete_user_key(ctx.email, provider)


# ── Chat ──────────────────────────────────────────────────────────────────────

@router.post("/ai/chat")
async def chat(body: ChatRequest, ctx: UserContext = Depends(require_min_role("Analyst"))):
    _check_rate(ctx.email)
    try:
        result = await ai_svc.chat(
            messages=[m.model_dump() for m in body.messages],
            context=body.context,
            user_email=ctx.email,
        )
        return result
    except ValueError as e:
        raise HTTPException(status_code=400, detail={"code": "ai_error", "message": str(e)})
    except Exception as e:
        # Surface provider errors clearly
        msg = str(e)
        if "401" in msg or "Unauthorized" in msg or "invalid_api_key" in msg.lower():
            raise HTTPException(status_code=400, detail={"code": "invalid_api_key", "message": "AI API key is invalid or expired. Check your key in Settings → AI Providers."})
        if "429" in msg:
            raise HTTPException(status_code=429, detail={"code": "provider_rate_limit", "message": "AI provider rate limit reached. Please wait and try again."})
        raise HTTPException(status_code=502, detail={"code": "ai_unavailable", "message": f"AI provider error: {msg[:200]}"})
