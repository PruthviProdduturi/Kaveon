"""Read-only Engine operations console, served through the platform bridge.

Studio is the only front door. The browser never holds an Engine credential:
each request carries the verified Kaveon principal and role to the Engine over
the server-side bridge, and the Engine applies its own ownership scoping to
query history. Nothing here mutates Engine state.
"""
from fastapi import APIRouter, Depends, HTTPException

from middleware import demo
from middleware.auth import UserContext
from middleware.permissions import require_min_role
from services import engine_bridge

router = APIRouter(tags=["engine-console"])


@router.get("/engine/quota")
def engine_quota(ctx: UserContext = Depends(require_min_role("Viewer"))):
    """The caller's demo posture: whether the platform is read-only for them
    (`read_only`), and their live-read quota on the coordinator (`quota`,
    null when no quota applies to them — an Admin, a self-hosted install, or
    a coordinator without the Engine integration). Never an error for a
    missing Engine: the Studio shows the counter only when there is one."""
    read_only = demo.enabled() and ctx.role != "Admin"
    try:
        engine = engine_bridge.quota(ctx.email, ctx.role)
    except HTTPException as error:
        if error.status_code in {502, 503, 504}:
            return {"demo": {"read_only": read_only, "engine": False}, "quota": None}
        raise
    quota = engine.get("quota")
    return {"demo": {"read_only": read_only, "engine": bool(engine["demo"].get("enabled"))},
            "quota": quota if isinstance(quota, dict) else None}


@router.get("/engine/console/cluster")
def console_cluster(ctx: UserContext = Depends(require_min_role("Viewer"))):
    result = engine_bridge.cluster(ctx.email, ctx.role)
    if not isinstance(result, dict):
        raise HTTPException(502, "Engine returned an invalid cluster record")
    return result


@router.get("/engine/console/queries")
def console_queries(ctx: UserContext = Depends(require_min_role("Viewer"))):
    result = engine_bridge.queries(ctx.email, ctx.role)
    if not isinstance(result, list):
        raise HTTPException(502, "Engine returned an invalid query listing")
    return result


@router.get("/engine/console/queries/{query_id}")
def console_query(query_id: str, ctx: UserContext = Depends(require_min_role("Viewer"))):
    result = engine_bridge.query(query_id, ctx.email, ctx.role)
    if result is None:
        raise HTTPException(404, "Query not found")
    if not isinstance(result, dict):
        raise HTTPException(502, "Engine returned an invalid query record")
    return result


@router.get("/engine/console/statistics")
def console_statistics(ctx: UserContext = Depends(require_min_role("Admin"))):
    result = engine_bridge.statistics(ctx.email, ctx.role)
    if not isinstance(result, dict) or not isinstance(result.get("statistics"), list):
        raise HTTPException(502, "Engine returned invalid statistics diagnostics")
    return result


@router.post("/engine/console/statistics/qualify")
def qualify_native_statistics(ctx: UserContext = Depends(require_min_role("Admin"))):
    if not engine_bridge.native_analyze_supported():
        raise HTTPException(409, "Connected Engine does not advertise native ANALYZE")
    result = engine_bridge.execute(
        'ANALYZE "ai_benchmarks"."leaderboard"', "OpenSource",
        ctx.email, ctx.role, schema="ai_benchmarks",
    )
    if not isinstance(result, dict):
        raise HTTPException(502, "Engine returned an invalid ANALYZE result")
    return {"capability": {"native_analyze": True}, "query": result}
