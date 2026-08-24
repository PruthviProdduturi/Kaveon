"""
DLM API — /api/v1/datasets/{dataset_id}/dlm/*

Compile and query a dataset's Data Language Model: the retrievable artifact
that resolves natural-language terms to columns/filters with no hosted LLM.

  POST /datasets/{id}/dlm/generate   compile (or refresh) the artifact
  GET  /datasets/{id}/dlm            artifact status + manifest/rollups
  GET  /datasets/{id}/freshness      data-change freshness score + rebuild recommendation
  GET  /datasets/{id}/dlm/resolve    term -> column/filter resolution (retrieval probe)
  GET  /dlm/route                    cross-dataset router: question -> which dataset(s)
  POST /dlm/serve-chart              dashboard chart from precomputed context (no live SQL)
"""

from typing import Any, Dict, List, Optional

from fastapi import APIRouter, HTTPException, Query, Depends
from pydantic import BaseModel

from middleware.auth import require_user_context, UserContext
from middleware.permissions import require_min_role
import services.dlm as dlm

router = APIRouter()


class AskBody(BaseModel):
    question: str
    limit: int = 50


class CurationBody(BaseModel):
    """Human overrides on a dataset's context spec. Any subset may be sent."""
    metrics: Dict[str, Any] = {}
    dimensions: Dict[str, Any] = {}
    value_aliases: Dict[str, str] = {}
    default_metric: Optional[str] = None


class ChartFilter(BaseModel):
    column: str
    operator: str = "="
    value: Any


class ServeChartMetric(BaseModel):
    column: str
    aggregation: str


class ServeChartBody(BaseModel):
    """A dashboard chart query: one or more metric aggregations, optional
    group-by and equality filters. Served from precomputed context when
    possible.  Send ``metrics`` for multi-metric charts (stacked bar, combo);
    ``metric_column``/``aggregation`` is kept for single-metric backward
    compat."""
    dataset_id: int
    metric_column: Optional[str] = None
    aggregation: Optional[str] = None
    metrics: Optional[List[ServeChartMetric]] = None
    group_by: Optional[str] = None
    filters: List[ChartFilter] = []


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


@router.get("/datasets/{dataset_id}/dlm/context")
def get_context(dataset_id: str, ctx: UserContext = Depends(require_user_context)):
    """The dataset's curatable context spec — auto-suggested defaults overlaid with
    human curation — for the context editor."""
    spec = dlm.get_context_spec(dataset_id)
    if not spec.get("ok"):
        raise HTTPException(status_code=404, detail=spec.get("reason", "no_artifact"))
    return spec


@router.put("/datasets/{dataset_id}/dlm/context")
def put_context(dataset_id: str, body: CurationBody,
                ctx: UserContext = Depends(require_min_role("Analyst"))):
    """Save human curation overrides (aliases, breakdowns, additivity, value aliases,
    default metric). Returns needs_regenerate=true when a breakdown/depth change means
    the precomputed answers must be rebuilt."""
    result = dlm.save_curation(dataset_id, body.dict())
    if not result.get("ok"):
        raise HTTPException(status_code=404, detail=result.get("reason", "save_failed"))
    return result


@router.get("/datasets/{dataset_id}/freshness")
def freshness(dataset_id: str, ctx: UserContext = Depends(require_user_context)):
    """How fresh is this dataset's DLM context? Combines time decay since the
    artifact was built with live data-change signals from pg_stat_user_tables.
    Returns a score in [0,1] and a recommendation: use_context / rebuild / no_context."""
    return dlm.check_freshness(dataset_id)


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
    assembled SQL, and chart hints — or ok=false if nothing matched.
    Triggers a background rebuild when the serving dataset's context is stale."""
    result = dlm.ask(body.question, limit=body.limit)
    dataset_id = result.get("dataset_id")
    if dataset_id and result.get("ok"):
        rebuilt = dlm.maybe_auto_rebuild(dataset_id)
        if rebuilt is True:
            result["_rebuild_triggered"] = True
    return result


@router.post("/dlm/serve-chart")
def serve_chart(body: ServeChartBody, ctx: UserContext = Depends(require_user_context)):
    """Serve a dashboard chart from precomputed DLM context — no live SQL.
    Returns served=true with columns/rows on a hit, served=false when the
    precomputed context can't answer (caller should fall back to /sql/execute)."""
    raw_filters = [f.dict() for f in body.filters] if body.filters else []

    metric_list = []
    if body.metrics:
        metric_list = [{"column": m.column, "aggregation": m.aggregation} for m in body.metrics]
    elif body.metric_column and body.aggregation:
        metric_list = [{"column": body.metric_column, "aggregation": body.aggregation}]

    if not metric_list:
        raise HTTPException(status_code=422, detail="No metric specified")

    if len(metric_list) == 1:
        mc = metric_list[0]
        return dlm.serve_chart(
            dataset_id=str(body.dataset_id),
            metric_column=mc["column"],
            aggregation=mc["aggregation"],
            group_by=body.group_by,
            filters=raw_filters,
        )

    return dlm.serve_chart_multi(
        dataset_id=str(body.dataset_id),
        metric_specs=metric_list,
        group_by=body.group_by,
        filters=raw_filters,
    )


@router.get("/dlm/filter-values")
def filter_values(dataset_id: int = Query(...), column: str = Query(...),
                  limit: int = Query(default=200, ge=1, le=1000),
                  ctx: UserContext = Depends(require_user_context)):
    """Distinct values for a dimension column, served from DLM context — no SQL."""
    return dlm.filter_values(str(dataset_id), column, limit=limit)


@router.post("/dashboards/{dashboard_id}/dlm/curate")
def curate_dashboard(dashboard_id: str,
                     ctx: UserContext = Depends(require_min_role("Analyst"))):
    """Precompute N-dim answer combos for a dashboard's filter×chart definitions.
    Multi-filter interactions serve instantly from context after curation."""
    result = dlm.curate_dashboard(dashboard_id)
    if not result.get("ok"):
        raise HTTPException(status_code=404, detail=result.get("reason", "curation_failed"))
    return result


@router.post("/datasets/{dataset_id}/dlm/refresh")
def incremental_refresh(dataset_id: str,
                        ctx: UserContext = Depends(require_min_role("Analyst"))):
    """Incrementally refresh DLM answers for new data — delta-merge for additive
    metrics, full recompute when delta is too large or metrics are non-additive."""
    result = dlm.incremental_refresh(dataset_id)
    if not result.get("ok"):
        raise HTTPException(status_code=404, detail=result.get("reason", "refresh_failed"))
    return result


@router.post("/dlm/cache/invalidate")
def invalidate_cache(
    dataset_id: Optional[str] = None,
    ctx: UserContext = Depends(require_min_role("Admin")),
):
    cleared = dlm.invalidate_caches(dataset_id)
    return {"ok": True, "cleared": cleared}


@router.get("/dlm/coverage")
def coverage(ctx: UserContext = Depends(require_user_context)):
    """What context is compiled and testable — datasets, date ranges, row counts,
    value coverage. Powers the homepage 'available context' banner."""
    items = dlm.coverage()
    return {"count": len(items), "datasets": items}
