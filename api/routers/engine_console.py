"""Read-only Engine operations console, served through the platform bridge.

Studio is the only front door. The browser never holds an Engine credential:
each request carries the verified Kaveon principal and role to the Engine over
the server-side bridge, and the Engine applies its own ownership scoping to
query history. Nothing here mutates Engine state.
"""
from fastapi import APIRouter, Depends, HTTPException

from middleware.auth import UserContext
from middleware.permissions import require_min_role
from services import engine_bridge

router = APIRouter(tags=["engine-console"])


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
