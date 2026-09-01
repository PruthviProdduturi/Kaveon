"""Charts router — /api/v1/charts."""

from fastapi import APIRouter, Response, HTTPException, Depends
from middleware.auth import require_auth, require_user_context, UserContext
from middleware.permissions import require_min_role, can_write, can_publish
from models.charts import ChartCreate, ChartUpdate
import services.charts as svc
import services.favorites as fav_svc
import services.user_recents as recents_svc

router = APIRouter()
NO_CACHE = {
    "Cache-Control": "no-cache, no-store, must-revalidate",
    "Pragma": "no-cache",
    "Expires": "0",
}


@router.get("/charts")
def list_charts(response: Response, ctx: UserContext = Depends(require_user_context)):
    response.headers.update(NO_CACHE)
    return svc.list_charts(ctx.email, ctx.role)


@router.get("/charts/summary")
def charts_summary(response: Response, ctx: UserContext = Depends(require_user_context)):
    response.headers.update(NO_CACHE)
    items = svc.list_charts(ctx.email, ctx.role)
    return {"count": len(items), "recent": items}


@router.get("/charts/{chart_id}")
def get_chart(chart_id: str, response: Response, ctx: UserContext = Depends(require_user_context)):
    response.headers.update(NO_CACHE)
    chart = svc.get_chart_by_id(chart_id, ctx.email, ctx.role)
    if not chart:
        raise HTTPException(status_code=404, detail="Chart not found")
    return chart


@router.post("/charts", status_code=201)
def create_chart(
    data: ChartCreate,
    ctx: UserContext = Depends(require_min_role("Analyst")),
):
    payload = data.model_dump(exclude_none=True)
    if payload.get("visibility") == "published" and not can_publish(ctx):
        payload["visibility"] = "internal"
    return svc.create_chart(payload, ctx.email)


@router.put("/charts/{chart_id}")
def update_chart(
    chart_id: str,
    data: ChartUpdate,
    ctx: UserContext = Depends(require_user_context),
):
    existing = svc.get_chart_by_id(chart_id)
    if not existing:
        raise HTTPException(status_code=404, detail="Chart not found")
    if not can_write(existing["created_by"], ctx):
        raise HTTPException(status_code=403, detail="You don't have permission to edit this chart")

    payload = data.model_dump(exclude_none=True)
    if payload.get("visibility") == "published" and not can_publish(ctx):
        payload["visibility"] = "internal"

    result = svc.update_chart(chart_id, payload)
    if not result:
        raise HTTPException(status_code=404, detail="Chart not found")
    return result


@router.patch("/charts/{chart_id}")
def patch_chart(
    chart_id: str,
    data: ChartUpdate,
    ctx: UserContext = Depends(require_user_context),
):
    existing = svc.get_chart_by_id(chart_id)
    if not existing:
        raise HTTPException(status_code=404, detail="Chart not found")
    if not can_write(existing["created_by"], ctx):
        raise HTTPException(status_code=403, detail="You don't have permission to edit this chart")

    payload = data.model_dump(exclude_none=True)
    if payload.get("visibility") == "published" and not can_publish(ctx):
        payload["visibility"] = "internal"

    result = svc.update_chart(chart_id, payload)
    if not result:
        raise HTTPException(status_code=404, detail="Chart not found")
    return result


@router.delete("/charts/{chart_id}", status_code=204)
def delete_chart(chart_id: str, ctx: UserContext = Depends(require_user_context)):
    existing = svc.get_chart_by_id(chart_id)
    if not existing:
        raise HTTPException(status_code=404, detail="Chart not found")
    if not can_write(existing["created_by"], ctx):
        raise HTTPException(status_code=403, detail="You don't have permission to delete this chart")
    svc.delete_chart(chart_id)
    # Also purge it from every user's recents (id is prefixed by type).
    try:
        recents_svc.remove_recent_all_users(f"chart-{chart_id}", "chart")
    except Exception as e:
        print(f"[Charts] recents cleanup failed for {chart_id}: {e}")


@router.put("/charts/{chart_id}/favorite")
def toggle_chart_favorite(chart_id: str, user: str = Depends(require_auth)):
    chart = svc.get_chart_by_id(chart_id)
    if not chart:
        raise HTTPException(status_code=404, detail="Chart not found")
    return fav_svc.toggle_favorite(
        {"object_type": "chart", "object_id": chart_id, "object_name": chart.get("name")},
        user,
    )
