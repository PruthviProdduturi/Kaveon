"""Charts router — /api/v1/charts."""

from fastapi import APIRouter, Request, Response, HTTPException, Depends
from middleware.auth import require_auth
import services.charts as svc
import services.favorites as fav_svc

router = APIRouter()
NO_CACHE = {
    "Cache-Control": "no-cache, no-store, must-revalidate",
    "Pragma": "no-cache",
    "Expires": "0",
}


@router.get("/charts")
def list_charts(response: Response, user: str = Depends(require_auth)):
    response.headers.update(NO_CACHE)
    return svc.list_charts()


@router.get("/charts/summary")
def charts_summary(response: Response, user: str = Depends(require_auth)):
    response.headers.update(NO_CACHE)
    items = svc.list_charts(user)
    return {"count": len(items), "recent": items}


@router.get("/charts/{chart_id}")
def get_chart(chart_id: str, response: Response, user: str = Depends(require_auth)):
    response.headers.update(NO_CACHE)
    chart = svc.get_chart_by_id(chart_id)
    if not chart:
        raise HTTPException(status_code=404, detail="Chart not found")
    return chart


@router.post("/charts", status_code=201)
def create_chart(data: dict, user: str = Depends(require_auth)):
    if not data.get("name") or not data.get("dataset_id") or not data.get("chart_type"):
        raise HTTPException(status_code=400, detail="Name, dataset_id, and chart_type are required")
    return svc.create_chart(data, user)


@router.put("/charts/{chart_id}")
def update_chart(chart_id: str, data: dict, user: str = Depends(require_auth)):
    result = svc.update_chart(chart_id, data)
    if not result:
        raise HTTPException(status_code=404, detail="Chart not found")
    return result


@router.patch("/charts/{chart_id}")
def patch_chart(chart_id: str, data: dict, user: str = Depends(require_auth)):
    result = svc.update_chart(chart_id, data)
    if not result:
        raise HTTPException(status_code=404, detail="Chart not found")
    return result


@router.delete("/charts/{chart_id}", status_code=204)
def delete_chart(chart_id: str, user: str = Depends(require_auth)):
    deleted = svc.delete_chart(chart_id)
    if not deleted:
        raise HTTPException(status_code=404, detail="Chart not found")


@router.put("/charts/{chart_id}/favorite")
def toggle_chart_favorite(chart_id: str, user: str = Depends(require_auth)):
    chart = svc.get_chart_by_id(chart_id)
    if not chart:
        raise HTTPException(status_code=404, detail="Chart not found")
    return fav_svc.toggle_favorite(
        {"object_type": "chart", "object_id": chart_id, "object_name": chart.get("name")},
        user,
    )
