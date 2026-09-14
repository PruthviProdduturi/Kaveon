"""
Adaptive context routing API — /api/v1/context/*

Endpoints exposing the staleness-scored context engine:

  POST /context/build     profile a database's schema into a context representation
  GET  /context/validity  per-element validity (staleness) report
  POST /context/ask       route a NL question: answer-from-context | hybrid | live query
"""

from typing import Optional

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel

from middleware.auth import require_user_context, UserContext
import dlm.profiler as profiler
import dlm.router as router_svc
import dlm.validity as validity
from services import postgresql_retirement_runtime

router = APIRouter()


def _require_legacy_context_store():
    if postgresql_retirement_runtime.requested():
        raise HTTPException(
            status_code=503,
            detail="The legacy PostgreSQL context profiler is unavailable after retirement; use dataset DLM endpoints.",
        )


class BuildBody(BaseModel):
    database: str
    schema_name: str = "public"
    tables: Optional[list[str]] = None


class AskBody(BaseModel):
    question: str
    database: str
    sql: Optional[str] = None            # deterministic parser's SQL for the live path
    schema_name: str = "public"
    threshold: float = validity.DEFAULT_THRESHOLD


@router.post("/context/build")
def build_context(body: BuildBody, ctx: UserContext = Depends(require_user_context)):
    """Generate the global context representation (statistical profile) for a
    database — no LLM, no data scan (reads pg_stats / pg_stat_user_tables)."""
    _require_legacy_context_store()
    return profiler.build_context(body.database, body.schema_name, body.tables)


@router.get("/context/validity")
def get_validity(database: str, schema_name: str = "public",
                 ctx: UserContext = Depends(require_user_context)):
    """Per-element validity report: score + factor breakdown for every profiled
    table and column, plus which elements have fallen below the routing threshold."""
    _require_legacy_context_store()
    return router_svc.validity_report(database, schema_name)


@router.post("/context/ask")
def ask(body: AskBody, ctx: UserContext = Depends(require_user_context)):
    """Route a natural-language question by the validity of the specific context
    elements it depends on. Returns the route taken (context/hybrid/query), the
    per-element validity that drove it, and the answer/result."""
    _require_legacy_context_store()
    return router_svc.ask(body.question, body.database, sql=body.sql,
                          schema=body.schema_name, threshold=body.threshold)
