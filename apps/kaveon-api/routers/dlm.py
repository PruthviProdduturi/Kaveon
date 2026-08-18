"""
DLM API — /api/v1/datasets/{dataset_id}/dlm/*

Compile and query a dataset's Data Language Model: the retrievable artifact
that resolves natural-language terms to columns/filters with no hosted LLM.

  POST /datasets/{id}/dlm/generate   compile (or refresh) the artifact
  GET  /datasets/{id}/dlm            artifact status + manifest/rollups
  GET  /datasets/{id}/dlm/resolve    term -> column/filter resolution (retrieval probe)
  GET  /dlm/route                    cross-dataset router: question -> which dataset(s)
"""

from fastapi import APIRouter, HTTPException, Query, Depends
from pydantic import BaseModel

from middleware.auth import require_user_context, UserContext
from middleware.permissions import require_min_role
import services.dlm as dlm

router = APIRouter()


class AskBody(BaseModel):
    question: str
    limit: int = 50


@router.post("/datasets/{dataset_id}/dlm/generate")
def generate(dataset_id: str, force: bool = Query(default=False),
             ctx: UserContext = Depends(require_min_role("Analyst"))):
    """Encode the dataset into its DLM artifact. Idempotent unless force=true."""
    result = dlm.generate_dlm(dataset_id, force=force)
    if not result.get("ok"):
        raise HTTPException(status_code=404, detail=result.get("reason", "generate_failed"))
    return result


@router.get("/datasets/{dataset_id}/dlm")
def status(dataset_id: str, ctx: UserContext = Depends(require_user_context)):
    """Return the compiled artifact, or 404 if it has not been generated yet."""
    art = dlm.get_dlm(dataset_id)
    if not art:
        raise HTTPException(status_code=404, detail="DLM not generated for this dataset")
    return art


@router.get("/datasets/{dataset_id}/dlm/resolve")
def resolve(dataset_id: str, term: str = Query(..., min_length=1),
            limit: int = Query(default=5, ge=1, le=50),
            ctx: UserContext = Depends(require_user_context)):
    """No-LLM retrieval probe: resolve a term to the column + filter it denotes."""
    return {"term": term, "matches": dlm.resolve_value(dataset_id, term, limit=limit)}


@router.get("/dlm/route")
def route(question: str = Query(..., min_length=1),
          limit: int = Query(default=3, ge=1, le=20),
          ctx: UserContext = Depends(require_user_context)):
    """CLM-over-CLMs: rank which dataset(s) a natural-language question targets."""
    return {"question": question, "datasets": dlm.route(question, limit=limit)}


@router.post("/dlm/ask")
def ask(body: AskBody, ctx: UserContext = Depends(require_user_context)):
    """Deterministic NL -> SQL via the DLM (no LLM). Returns the routed dataset,
    assembled SQL, and chart hints — or ok=false if nothing matched."""
    return dlm.ask(body.question, limit=body.limit)


@router.get("/dlm/coverage")
def coverage(ctx: UserContext = Depends(require_user_context)):
    """What context is compiled and testable — datasets, date ranges, row counts,
    value coverage. Powers the homepage 'available context' banner."""
    items = dlm.coverage()
    return {"count": len(items), "datasets": items}
