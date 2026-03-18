"""Metadata summary router — GET /api/v1/metadata/summary."""

import os
import concurrent.futures
from fastapi import APIRouter, Response, HTTPException, Depends

from middleware.auth import require_auth
import services.datasets as datasets_svc
import services.charts as charts_svc
import services.dashboards as dashboards_svc
import services.favorites as favorites_svc
import services.saved_queries as saved_queries_svc

router = APIRouter()
NO_CACHE = {"Cache-Control": "no-cache, no-store, must-revalidate", "Pragma": "no-cache", "Expires": "0"}


@router.get("/metadata/summary")
def metadata_summary(response: Response, user: str = Depends(require_auth)):
    response.headers.update(NO_CACHE)

    # Return empty payload while app is in setup mode — the frontend shows the setup overlay
    db_type = os.environ.get("METADATA_DB_TYPE") or "fabric_sql"
    has_connection = os.environ.get("METADATA_ENDPOINT") if db_type in ("fabric_sql", "azure_sql") else os.environ.get("METADATA_HOST")
    if not has_connection or not os.environ.get("METADATA_DATABASE"):
        return {"datasets": [], "charts": [], "dashboards": [], "favorites": [], "savedQueries": []}

    # Fetch all in parallel using threads (pyodbc is synchronous)
    with concurrent.futures.ThreadPoolExecutor(max_workers=5) as executor:
        f_datasets = executor.submit(datasets_svc.list_datasets)
        f_charts = executor.submit(charts_svc.list_charts)
        f_dashboards = executor.submit(dashboards_svc.list_dashboards)
        f_favorites = executor.submit(favorites_svc.list_favorites, user)
        f_saved = executor.submit(saved_queries_svc.list_saved_queries, user)

        datasets = f_datasets.result()
        charts = f_charts.result()
        dashboards = f_dashboards.result()
        favorites = f_favorites.result()
        saved_queries = f_saved.result()

    return {
        "datasets": datasets,
        "charts": charts,
        "dashboards": dashboards,
        "favorites": favorites,
        "savedQueries": saved_queries,
    }
