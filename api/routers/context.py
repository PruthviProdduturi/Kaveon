"""
Adaptive context routing API — /api/v1/context/*

Endpoints exposing the staleness-scored context engine:

  POST /context/build     profile a database's schema into a context representation
  GET  /context/validity  per-element validity (staleness) report
  POST /context/ask       route a NL question: answer-from-context | hybrid | live query
"""

from typing import Optional

from fastapi import APIRouter, Depends
from pydantic import BaseModel

from middleware.auth import require_user_context, UserContext
import dlm.profiler as profiler
import dlm.router as router_svc
import dlm.validity as validity

router = APIRouter()


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
    return profiler.build_context(body.database, body.schema_name, body.tables)


@router.get("/context/validity")
def get_validity(database: str, schema_name: str = "public",
                 ctx: UserContext = Depends(require_user_context)):
    """Per-element validity report: score + factor breakdown for every profiled
    table and column, plus which elements have fallen below the routing threshold."""
    return router_svc.validity_report(database, schema_name)


@router.post("/context/ask")
def ask(body: AskBody, ctx: UserContext = Depends(require_user_context)):
    """Route a natural-language question by the validity of the specific context
    elements it depends on. Returns the route taken (context/hybrid/query), the
    per-element validity that drove it, and the answer/result."""
    return router_svc.ask(body.question, body.database, sql=body.sql,
                          schema=body.schema_name, threshold=body.threshold)
