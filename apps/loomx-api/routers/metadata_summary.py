"""Metadata summary router — GET /api/v1/metadata/summary."""

from fastapi import APIRouter, Request
from fastapi.responses import Response
import concurrent.futures

from middleware.auth import get_user_email
import services.datasets as datasets_svc
import services.charts as charts_svc
import services.dashboards as dashboards_svc
import services.favorites as favorites_svc
import services.saved_queries as saved_queries_svc

router = APIRouter()


@router.get("/metadata/summary")
def metadata_summary(request: Request, response: Response):
    response.headers["Cache-Control"] = "no-cache, no-store, must-revalidate"
    response.headers["Pragma"] = "no-cache"
    response.headers["Expires"] = "0"

    user_email = get_user_email(request)

    # Fetch all in parallel using threads (pyodbc is synchronous)
    with concurrent.futures.ThreadPoolExecutor(max_workers=5) as executor:
        f_datasets = executor.submit(datasets_svc.list_datasets)
        f_charts = executor.submit(charts_svc.list_charts)
        f_dashboards = executor.submit(dashboards_svc.list_dashboards)
        f_favorites = executor.submit(favorites_svc.list_favorites, user_email)
        f_saved = executor.submit(saved_queries_svc.list_saved_queries, user_email)

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
