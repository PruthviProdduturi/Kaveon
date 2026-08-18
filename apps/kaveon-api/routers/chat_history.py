"""
Chat History API — /api/v1/chat/history

CRUD for persisted chat sessions and messages.
Each session belongs to a user (by email) and contains an ordered list of messages.
"""

from typing import Optional
from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel

from middleware.auth import require_user_context, UserContext
import database.metadata as db

router = APIRouter()

MAX_LIMIT = 1000
DEFAULT_LIMIT = 50


# ── Request/Response types ────────────────────────────────────────────────────

class SessionCreate(BaseModel):
    title: Optional[str] = "New conversation"


class SessionOut(BaseModel):
    id: int
    title: str
    created_at: str
    updated_at: str


class MessageCreate(BaseModel):
    role: str  # 'user' | 'assistant'
    content: str
    sql_query: Optional[str] = None
    chart_type: Optional[str] = None
    data: Optional[dict] = None
    route: Optional[str] = None


class MessageOut(BaseModel):
    id: int
    session_id: int
    role: str
    content: str
    sql_query: Optional[str] = None
    chart_type: Optional[str] = None
    data: Optional[dict] = None
    route: Optional[str] = None
    created_at: str


# ── Helpers ───────────────────────────────────────────────────────────────────

def _assert_session_owner(session_id: int, email: str) -> dict:
    """Return the session row or raise 404."""
    row = db.query_one(
        "SELECT id, user_email, title, created_at, updated_at "
        "FROM dbo.chat_sessions WHERE id = @param0",
        [session_id],
    )
    if not row or row["user_email"] != email:
        raise HTTPException(
            status_code=404,
            detail={"code": "not_found", "message": "Session not found."},
        )
    return row


# ── Endpoints ─────────────────────────────────────────────────────────────────

@router.get("/chat/history")
def list_sessions(
    limit: int = DEFAULT_LIMIT,
    offset: int = 0,
    ctx: UserContext = Depends(require_user_context),
):
    """List chat sessions for the current user, newest first."""
    limit = min(max(limit, 1), MAX_LIMIT)
    offset = max(offset, 0)

    rows = db.query(
        "SELECT id, title, created_at, updated_at "
        "FROM dbo.chat_sessions "
        "WHERE user_email = @param0 "
        "ORDER BY updated_at DESC "
        "LIMIT @param1 OFFSET @param2",
        [ctx.email, limit, offset],
    )
    return {"sessions": rows["rows"], "count": rows["row_count"]}


@router.get("/chat/history/{session_id}")
def get_session(session_id: int, ctx: UserContext = Depends(require_user_context)):
    """Return all messages in a session."""
    session = _assert_session_owner(session_id, ctx.email)

    msgs = db.query(
        "SELECT id, session_id, role, content, sql_query, chart_type, data, route, created_at "
        "FROM dbo.chat_messages "
        "WHERE session_id = @param0 "
        "ORDER BY created_at",
        [session_id],
    )
    return {
        "session": {
            "id": session["id"],
            "title": session["title"],
            "created_at": session["created_at"],
            "updated_at": session["updated_at"],
        },
        "messages": msgs["rows"],
    }


@router.post("/chat/history", status_code=201)
def create_session(body: SessionCreate, ctx: UserContext = Depends(require_user_context)):
    """Create a new chat session."""
    title = (body.title or "New conversation").strip()[:500]

    row = db.query_one(
        "INSERT INTO dbo.chat_sessions (user_email, title) "
        "VALUES (@param0, @param1) "
        "RETURNING id, title, created_at, updated_at",
        [ctx.email, title],
    )
    return row


@router.post("/chat/history/{session_id}/messages", status_code=201)
def add_message(
    session_id: int,
    body: MessageCreate,
    ctx: UserContext = Depends(require_user_context),
):
    """Append a message to a session."""
    _assert_session_owner(session_id, ctx.email)

    if body.role not in ("user", "assistant"):
        raise HTTPException(
            status_code=400,
            detail={"code": "invalid_role", "message": "Role must be 'user' or 'assistant'."},
        )

    import json
    data_json = json.dumps(body.data) if body.data else None

    row = db.query_one(
        "INSERT INTO dbo.chat_messages (session_id, role, content, sql_query, chart_type, data, route) "
        "VALUES (@param0, @param1, @param2, @param3, @param4, @param5::jsonb, @param6) "
        "RETURNING id, session_id, role, content, sql_query, chart_type, data, route, created_at",
        [session_id, body.role, body.content, body.sql_query, body.chart_type, data_json, body.route],
    )

    # Touch session updated_at
    db.execute(
        "UPDATE dbo.chat_sessions SET updated_at = NOW() WHERE id = @param0",
        [session_id],
    )

    return row


@router.delete("/chat/history/{session_id}", status_code=204)
def delete_session(session_id: int, ctx: UserContext = Depends(require_user_context)):
    """Delete a session and its messages (CASCADE)."""
    _assert_session_owner(session_id, ctx.email)
    db.execute("DELETE FROM dbo.chat_sessions WHERE id = @param0", [session_id])
