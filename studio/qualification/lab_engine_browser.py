"""Browser regression for the Catalog: definition page, then SQL Lab as its query mode.

Walks Catalog -> OpenSource -> nyc_taxi -> green_trips, checks the definition,
hands off to the query mode with the prefilled SELECT, runs it, and checks the result.

Run a local Studio dev server on port 3001 with KAVEON_DEV_USER_EMAIL set, then:
    python studio/qualification/lab_engine_browser.py
"""
from pathlib import Path
import os
from playwright.sync_api import sync_playwright

ROOT = Path(__file__).resolve().parents[2]
SCREENSHOT = ROOT / "tmp" / "lab-engine-browser.png"
SCREENSHOT.parent.mkdir(exist_ok=True)
BASE = os.environ.get("STUDIO_TEST_URL", "http://localhost:3001")


def json_route(route, request):
    path = request.url.split("/api/kaveon", 1)[-1]
    if path == "/api/v1/lab/engine/sources":
        return route.fulfill(json={"success": True, "sources": [{"id": "source-1", "name": "OpenSource", "catalog": "OpenSource"}]})
    if path == "/api/v1/lab/engine/source-1/schemas":
        return route.fulfill(json={"success": True, "schemas": ["covid", "nyc_taxi", "ai_benchmarks"]})
    if path.endswith("/schemas/covid/tables") or path.endswith("/schemas/ai_benchmarks/tables"):
        return route.fulfill(json={"success": True, "tables": []})
    if path == "/api/v1/lab/engine/source-1/schemas/nyc_taxi/tables":
        return route.fulfill(json={"success": True, "tables": ["green_trips"]})
    if path.endswith("/schemas/nyc_taxi/tables/green_trips/columns"):
        return route.fulfill(json={"success": True, "schema": {"columns": [{"name": "trip_distance", "dataType": "Float64", "isNullable": False}]}})
    if path == "/api/v1/catalog/source-1/schemas/nyc_taxi/tables/green_trips":
        return route.fulfill(json={"success": True, "table": {"catalog": "OpenSource", "schema": "nyc_taxi", "name": "green_trips",
                                   "location": "silver/green_trips/part-00000.parquet", "access": "Shortcut", "format": "Parquet", "revision": 2, "lifecycle": "active",
                                   "columns": [{"name": "trip_distance", "dataType": "Float64", "isNullable": False}]}})
    if path == "/api/v1/catalog/source-1/schemas/nyc_taxi/tables/green_trips/usage":
        return route.fulfill(json={"success": True, "datasets": [], "charts": [], "dashboards": [], "dlm": []})
    if path == "/api/v1/engine/console/statistics":
        return route.fulfill(status=403, json={"detail": "admin only"})
    if path == "/api/v1/lab/query":
        body = request.post_data_json
        assert body["engineSourceId"] == "source-1"
        assert body["engineSchema"] == "nyc_taxi"
        assert "database" not in body
        return route.fulfill(json={"success": True, "columns": ["trip_distance"], "rows": [[42]], "rowCount": 1, "executionTime": 3})
    if path == "/api/v1/data-sources/active":
        return route.fulfill(json={"dataSources": []})
    if path.startswith("/api/v1/lab/query-history"):
        return route.fulfill(json=[])
    return route.fulfill(json={"success": True, "tables": []})


with sync_playwright() as p:
    browser = p.chromium.launch(channel="msedge", headless=True)
    page = browser.new_page(viewport={"width": 1440, "height": 960})
    page.route(BASE + "/api/auth/session", lambda route: route.fulfill(json={"user": {"email": "test@localhost"}, "expires": "2099-01-01T00:00:00.000Z"}))
    page.route(BASE + "/api/kaveon/api/v1/**", json_route)
    errors = []
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.goto(BASE + "/catalog/OpenSource/nyc_taxi/green_trips", wait_until="domcontentloaded")
    # The shell's tree opens along the URL; the definition page shows the typed column.
    page.get_by_role("heading", name="green_trips").wait_for(state="visible", timeout=20_000)
    for schema in ["covid", "nyc_taxi", "ai_benchmarks"]:
        page.locator("aside").get_by_text(schema, exact=True).wait_for()
    assert page.locator("aside").get_by_text("green_trips", exact=True).count() >= 1
    assert page.get_by_text("Float64", exact=True).first.is_visible()
    assert page.get_by_text("relative to the catalog", exact=False).is_visible()
    # Hand off to the query mode: one tree, the workbench in the pane, SELECT prefilled.
    page.get_by_role("link", name="Query in SQL Lab").click()
    page.wait_for_url("**/catalog/query?*", timeout=20_000)
    page.locator(".monaco-editor").wait_for(state="visible", timeout=20_000)
    assert page.locator("aside").get_by_text("nyc_taxi", exact=True).is_visible()
    assert page.get_by_label("Source").count() == 0
    page.wait_for_function("window.monaco && window.monaco.editor.getModels().some(m => m.getValue().includes('green_trips'))", timeout=15_000)
    page.get_by_title("Run query (Ctrl+Enter)").click()
    page.locator("th").get_by_text("trip_distance", exact=True).wait_for(state="visible")
    assert page.get_by_text("42", exact=True).is_visible()
    assert not errors, errors
    page.screenshot(path=str(SCREENSHOT), full_page=True)
    browser.close()

print("PASS: Catalog definition -> SQL Lab query mode -> prefilled SELECT -> result")
