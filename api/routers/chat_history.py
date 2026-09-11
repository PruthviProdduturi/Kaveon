"""
Chat History API — /api/v1/chat/history

CRUD for persisted chat sessions and messages.
Each session belongs to a user (by email) and contains an ordered list of messages.
"""

from typing import Optional
import os
from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel

from middleware.auth import require_user_context, UserContext
import database.metadata as db
from services import product_outbox
from services.chat_history_backfill import session_document,message_document

router = APIRouter()

MAX_LIMIT = 1000
DEFAULT_LIMIT = 50
MAX_DELETE_MESSAGES=1_000
def _enabled():return os.getenv("KAVEON_CHAT_HISTORY_OUTBOX_ENABLED")=="true"


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
    try:
        from services import product_shadow_read
        product_shadow_read.observe_chat_session(session,msgs["rows"],ctx.email)
    except Exception:
        pass
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

    if _enabled():
     with db.transaction() as tx:
      row=tx.query_one(
        "INSERT INTO dbo.chat_sessions (user_email, title) VALUES (@param0, @param1) RETURNING id,user_email,title,created_at,updated_at",[ctx.email,title])
      product_outbox.enqueue(tx,family="chat_sessions",operation="create",record_id=str(row["id"]),payload=session_document(row),actor=ctx.email,owner=ctx.email)
     return row
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

    if _enabled():
     with db.transaction() as tx:
      session=tx.query_one("SELECT id,user_email,title,created_at,updated_at FROM dbo.chat_sessions WHERE id=@param0 AND user_email=@param1 FOR UPDATE",[session_id,ctx.email])
      if not session:raise HTTPException(404,{"code":"not_found","message":"Session not found."})
      row=tx.query_one("INSERT INTO dbo.chat_messages (session_id,role,content,sql_query,chart_type,data,route) VALUES (@param0,@param1,@param2,@param3,@param4,@param5::jsonb,@param6) RETURNING id,session_id,role,content,sql_query,chart_type,data,route,created_at",[session_id,body.role,body.content,body.sql_query,body.chart_type,data_json,body.route]);row={**row,"user_email":ctx.email}
      updated=tx.query_one("UPDATE dbo.chat_sessions SET updated_at=NOW() WHERE id=@param0 RETURNING id,user_email,title,created_at,updated_at",[session_id])
      product_outbox.enqueue(tx,family="chat_messages",operation="create",record_id=str(row["id"]),payload=message_document(row),actor=ctx.email,owner=ctx.email)
      product_outbox.enqueue(tx,family="chat_sessions",operation="update",record_id=str(session_id),payload=session_document(updated),actor=ctx.email,owner=ctx.email)
     return row
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
    if _enabled():
     with db.transaction() as tx:
      session=tx.query_one("SELECT id,user_email FROM dbo.chat_sessions WHERE id=@param0 AND user_email=@param1 FOR UPDATE",[session_id,ctx.email])
      if not session:raise HTTPException(404,{"code":"not_found","message":"Session not found."})
      messages=tx.query("SELECT id FROM dbo.chat_messages WHERE session_id=@param0 ORDER BY id LIMIT @param1 FOR UPDATE",[session_id,MAX_DELETE_MESSAGES+1])["rows"]
      if len(messages)>MAX_DELETE_MESSAGES:raise HTTPException(409,"Chat session exceeds bounded migration delete")
      tx.execute("DELETE FROM dbo.chat_sessions WHERE id=@param0",[session_id])
      for message in messages:product_outbox.enqueue(tx,family="chat_messages",operation="delete",record_id=str(message["id"]),payload={},actor=ctx.email,owner=ctx.email)
      product_outbox.enqueue(tx,family="chat_sessions",operation="delete",record_id=str(session_id),payload={},actor=ctx.email,owner=ctx.email)
     return
    db.execute("DELETE FROM dbo.chat_sessions WHERE id = @param0", [session_id])
