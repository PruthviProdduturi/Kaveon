"""Dashboards router — /api/v1/dashboards."""

import hashlib

from fastapi import APIRouter, Response, HTTPException, Depends
from middleware.auth import require_auth, require_user_context, UserContext
from middleware.permissions import require_min_role, can_write, can_publish
from middleware.demo import allowed_in_demo
from models.dashboards import DashboardCreate, DashboardUpdate, DashboardFavoriteBody
import services.dashboards as svc
import services.thumbnails as thumbnails
import services.favorites as fav_svc
import services.user_recents as recents_svc

router = APIRouter()
NO_CACHE = {
    "Cache-Control": "no-cache, no-store, must-revalidate",
    "Pragma": "no-cache",
    "Expires": "0",
}


@router.get("/dashboards")
def list_dashboards(response: Response, ctx: UserContext = Depends(require_user_context)):
    response.headers.update(NO_CACHE)
    return svc.list_dashboards(ctx.email, ctx.role)


@router.get("/dashboards/summary")
def dashboards_summary(response: Response, ctx: UserContext = Depends(require_user_context)):
    response.headers.update(NO_CACHE)
    items = svc.list_dashboards(ctx.email, ctx.role)
    return {"count": len(items), "recent": items}


@router.get("/dashboards/{dashboard_id}")
def get_dashboard(dashboard_id: str, response: Response, ctx: UserContext = Depends(require_user_context)):
    response.headers.update(NO_CACHE)
    dashboard = svc.get_dashboard_by_id(dashboard_id, ctx.email, ctx.role)
    if not dashboard:
        raise HTTPException(status_code=404, detail="Dashboard not found")
    is_fav = fav_svc.is_favorite(ctx.email, "dashboard", dashboard_id)
    return {**dashboard, "is_favorite": is_fav}


@router.get("/dashboards/{dashboard_id}/thumbnail")
def get_dashboard_thumbnail(
    dashboard_id: str,
    theme: str = "light",
    ctx: UserContext = Depends(require_user_context),
):
    """Serve the Library preview for one theme as an image rather than as JSON.

    A dashboard is captured once per theme; when only one capture exists it is
    served for both, so a dashboard never falls back to a placeholder merely
    because the viewer flipped themes. No preview at all answers 404.
    """
    stored = svc.get_dashboard_thumbnail(dashboard_id, ctx.email, ctx.role, theme == "dark")
    if not stored:
        raise HTTPException(status_code=404, detail="Dashboard thumbnail not found")
    try:
        content, media_type = thumbnails.decode(stored)
    except ValueError as error:
        raise HTTPException(status_code=422, detail=str(error)) from None
    return Response(content=content, media_type=media_type, headers={
        "Cache-Control": "private, max-age=60, must-revalidate",
        "ETag": f'"{hashlib.sha256(content).hexdigest()[:32]}"',
    })


@router.post("/dashboards", status_code=201)
def create_dashboard(
    data: DashboardCreate,
    ctx: UserContext = Depends(require_min_role("Analyst")),
):
    payload = data.model_dump(exclude_none=True)
    if payload.get("visibility") == "published" and not can_publish(ctx):
        payload["visibility"] = "internal"
    return svc.create_dashboard(payload, ctx.email)


@router.put("/dashboards/{dashboard_id}")
def update_dashboard(
    dashboard_id: str,
    data: DashboardUpdate,
    ctx: UserContext = Depends(require_user_context),
):
    existing = svc.get_dashboard_by_id(dashboard_id)
    if not existing:
        raise HTTPException(status_code=404, detail="Dashboard not found")
    if not can_write(existing["created_by"], ctx):
        raise HTTPException(status_code=403, detail="You don't have permission to edit this dashboard")

    payload = data.model_dump(exclude_none=True)
    if payload.get("visibility") == "published" and not can_publish(ctx):
        payload["visibility"] = "internal"

    result = svc.update_dashboard(dashboard_id, payload, ctx.email, ctx.role)
    if not result:
        raise HTTPException(status_code=404, detail="Dashboard not found")
    return result


@router.delete("/dashboards/{dashboard_id}", status_code=204)
def delete_dashboard(dashboard_id: str, ctx: UserContext = Depends(require_user_context)):
    existing = svc.get_dashboard_by_id(dashboard_id)
    if not existing:
        raise HTTPException(status_code=404, detail="Dashboard not found")
    if not can_write(existing["created_by"], ctx):
        raise HTTPException(status_code=403, detail="You don't have permission to delete this dashboard")
    svc.delete_dashboard(dashboard_id, ctx.email, ctx.role)
    # Also purge it from every user's recents so it doesn't linger there.
    # Recents store the id prefixed by type (e.g. "dashboard-<id>").
    try:
        recents_svc.remove_recent_all_users(f"dashboard-{dashboard_id}", "dashboard")
    except Exception as e:
        print(f"[Dashboards] recents cleanup failed for {dashboard_id}: {e}")


@router.put("/dashboards/{dashboard_id}/favorite")
@allowed_in_demo
def set_dashboard_favorite(dashboard_id: str, data: DashboardFavoriteBody, user: str = Depends(require_auth)):
    dashboard = svc.get_dashboard_by_id(dashboard_id)
    if not dashboard:
        raise HTTPException(status_code=404, detail="Dashboard not found")

    if data.is_favorite:
        favorite = fav_svc.create_favorite(
            {"object_type": "dashboard", "object_id": dashboard_id, "object_name": dashboard.get("name")},
            user,
        )
        return {"favorited": True, "favorite": favorite}
    else:
        fav_svc.delete_favorite(user, "dashboard", dashboard_id)
        return {"favorited": False}
