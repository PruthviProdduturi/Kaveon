"""Dashboards router — /api/v1/dashboards."""

from fastapi import APIRouter, Request, Response, HTTPException, Depends
from middleware.auth import require_auth
from models.dashboards import DashboardCreate, DashboardUpdate, DashboardFavoriteBody
import services.dashboards as svc
import services.favorites as fav_svc

router = APIRouter()
NO_CACHE = {
    "Cache-Control": "no-cache, no-store, must-revalidate",
    "Pragma": "no-cache",
    "Expires": "0",
}


@router.get("/dashboards")
def list_dashboards(response: Response, user: str = Depends(require_auth)):
    response.headers.update(NO_CACHE)
    return svc.list_dashboards()


@router.get("/dashboards/summary")
def dashboards_summary(response: Response, user: str = Depends(require_auth)):
    response.headers.update(NO_CACHE)
    items = svc.list_dashboards(user)
    return {"count": len(items), "recent": items}


@router.get("/dashboards/{dashboard_id}")
def get_dashboard(dashboard_id: str, response: Response, user: str = Depends(require_auth)):
    response.headers.update(NO_CACHE)
    dashboard = svc.get_dashboard_by_id(dashboard_id)
    if not dashboard:
        raise HTTPException(status_code=404, detail="Dashboard not found")
    is_fav = fav_svc.is_favorite(user, "dashboard", dashboard_id)
    return {**dashboard, "is_favorite": is_fav}


@router.post("/dashboards", status_code=201)
def create_dashboard(data: DashboardCreate, user: str = Depends(require_auth)):
    return svc.create_dashboard(data.model_dump(exclude_none=True), user)


@router.put("/dashboards/{dashboard_id}")
def update_dashboard(dashboard_id: str, data: DashboardUpdate, user: str = Depends(require_auth)):
    result = svc.update_dashboard(dashboard_id, data.model_dump(exclude_none=True))
    if not result:
        raise HTTPException(status_code=404, detail="Dashboard not found")
    return result


@router.delete("/dashboards/{dashboard_id}", status_code=204)
def delete_dashboard(dashboard_id: str, user: str = Depends(require_auth)):
    deleted = svc.delete_dashboard(dashboard_id)
    if not deleted:
        raise HTTPException(status_code=404, detail="Dashboard not found")


@router.put("/dashboards/{dashboard_id}/favorite")
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
