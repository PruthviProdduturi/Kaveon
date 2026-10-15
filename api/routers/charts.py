"""Charts router — /api/v1/charts."""

import hashlib

from fastapi import APIRouter, Response, HTTPException, Depends
from middleware.auth import require_auth, require_user_context, UserContext
from middleware.permissions import require_min_role, can_write, can_publish
from middleware.demo import allowed_in_demo
from models.charts import ChartCreate, ChartUpdate
import services.charts as svc
import services.thumbnails as thumbnails
import services.favorites as fav_svc
import services.user_recents as recents_svc
from routers import reference_guard

router = APIRouter()
NO_CACHE = {
    "Cache-Control": "no-cache, no-store, must-revalidate",
    "Pragma": "no-cache",
    "Expires": "0",
}


def _chart_references(dashboard: dict) -> set[str]:
    """The chart ids one dashboard document names.

    `chart_revisions` is what the Engine derives the dashboard's references
    from, so it is the authority on which charts the catalog will refuse to
    delete; `charts` is read as well so a document written before that field
    existed still accounts for its tiles.
    """
    revisions = dashboard.get("chart_revisions")
    named = set(revisions) if isinstance(revisions, dict) else set()
    charts = dashboard.get("charts")
    if isinstance(charts, (list, tuple)):
        named |= {str(item) for item in charts if item is not None}
    return {str(item) for item in named}


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


@router.get("/charts/{chart_id}/thumbnail")
def get_chart_thumbnail(
    chart_id: str,
    ctx: UserContext = Depends(require_user_context),
):
    """Serve the Library preview as an image rather than as JSON.

    The card points an <img> here, so the browser caches the bytes and the
    chart list stays small. A chart with no preview answers 404 and the card
    draws its placeholder.
    """
    stored = svc.get_chart_thumbnail(chart_id, ctx.email, ctx.role)
    if not stored:
        raise HTTPException(status_code=404, detail="Chart thumbnail not found")
    try:
        content, media_type = thumbnails.decode(stored)
    except ValueError as error:
        raise HTTPException(status_code=422, detail=str(error)) from None
    return Response(content=content, media_type=media_type, headers={
        "Cache-Control": "private, max-age=60, must-revalidate",
        "ETag": f'"{hashlib.sha256(content).hexdigest()[:32]}"',
    })


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

    result = svc.update_chart(chart_id, payload, ctx.email, ctx.role)
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

    result = svc.update_chart(chart_id, payload, ctx.email, ctx.role)
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
    # A dashboard names every chart on it as a reference (its
    # `chart_revisions`), so a chart on any dashboard could not be deleted at
    # all: the catalog refused the transaction and the caller was handed the
    # Engine's own words — a 502 quoting `product record 'dashboard/<id>'
    # references missing 'chart/<id>'`. Clearing the reference is not this
    # request's business either, because removing a tile edits someone else's
    # dashboard. So the delete is refused with the dashboards named, which is
    # something the caller can act on. Counted at system scope: a dashboard
    # the caller cannot see still blocks the delete, and is counted without
    # being named.
    total, names = reference_guard.dependents(
        "dashboards", lambda document: str(chart_id) in _chart_references(document),
        actor=ctx.email, role=ctx.role)
    if total:
        raise HTTPException(status_code=409, detail=reference_guard.message(
            total, names, "dashboard",
            "Remove this chart from them before deleting it."))
    # References first, then the record.
    #
    # A recents entry and a favorite each NAME this chart as a reference in
    # the product catalog, and the catalog refuses to delete a record that
    # another record still points at:
    #
    #     invalid transaction: product record 'user_recent/<id>'
    #     references missing 'chart/<id>'
    #
    # These purges used to run after the delete, so the delete raised and they
    # never ran at all: any chart anyone had ever opened or favorited could
    # not be deleted, and the Studio reported it as a permission problem. A
    # failure to clear the references is raised rather than printed, because
    # proceeding would only produce that same rejection one line later.
    try:
        recents_svc.remove_recent_all_users(f"chart-{chart_id}", "chart")
        fav_svc.remove_favorite_all_users("chart", chart_id)
    except Exception as error:
        raise HTTPException(
            status_code=502,
            detail=f"Could not clear this chart's references, so it was not deleted: {error}",
        ) from error
    svc.delete_chart(chart_id, ctx.email, ctx.role)


@router.put("/charts/{chart_id}/favorite")
@allowed_in_demo
def toggle_chart_favorite(chart_id: str, user: str = Depends(require_auth)):
    chart = svc.get_chart_by_id(chart_id)
    if not chart:
        raise HTTPException(status_code=404, detail="Chart not found")
    return fav_svc.toggle_favorite(
        {"object_type": "chart", "object_id": chart_id, "object_name": chart.get("name")},
        user,
    )
