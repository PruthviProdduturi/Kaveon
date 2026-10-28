"""Metadata summary router — GET /api/v1/metadata/summary."""

import concurrent.futures
from fastapi import APIRouter, Response, HTTPException, Depends

from middleware.auth import require_user_context, UserContext
import services.datasets as datasets_svc
import services.charts as charts_svc
import services.dashboards as dashboards_svc
import services.favorites as favorites_svc
import services.saved_queries as saved_queries_svc

router = APIRouter()
NO_CACHE = {"Cache-Control": "no-cache, no-store, must-revalidate", "Pragma": "no-cache", "Expires": "0"}


@router.get("/metadata/summary")
def metadata_summary(response: Response, ctx: UserContext = Depends(require_user_context)):
    response.headers.update(NO_CACHE)

    # There was a setup-mode guard here that returned an empty payload while
    # METADATA_* was unconfigured, so the frontend could show a setup overlay.
    # Those variables configured an external metadata database that no longer
    # exists; the services below read KaveonDB, which is ready whenever the
    # Engine is.

    # Fetch all in parallel using threads (pyodbc is synchronous)
    with concurrent.futures.ThreadPoolExecutor(max_workers=5) as executor:
        f_datasets = executor.submit(datasets_svc.list_datasets, ctx.email, ctx.role)
        f_charts = executor.submit(charts_svc.list_charts, ctx.email, ctx.role)
        f_dashboards = executor.submit(dashboards_svc.list_dashboards, ctx.email, ctx.role)
        f_favorites = executor.submit(favorites_svc.list_favorites, ctx.email)
        f_saved = executor.submit(saved_queries_svc.list_saved_queries, ctx.email)

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
