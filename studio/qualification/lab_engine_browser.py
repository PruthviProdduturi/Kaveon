"""Browser regression for SQL Lab's server-resolved Engine catalog flow.

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
    page.goto(BASE + "/lab", wait_until="domcontentloaded")
    source = page.get_by_label("Source")
    source.wait_for(state="visible", timeout=20_000)
    source.select_option("kaveon")
    catalog = page.get_by_label("Catalog")
    catalog.wait_for(state="visible", timeout=10_000)
    catalog.select_option("source-1")
    for schema in ["covid", "nyc_taxi", "ai_benchmarks"]:
        page.locator(".schema-header").get_by_text(schema, exact=True).wait_for()
    nyc_schema = page.locator(".schema-header").filter(has=page.get_by_text("nyc_taxi", exact=True))
    nyc_schema.get_by_text("1 tables", exact=True).wait_for()
    nyc_schema.click()
    page.get_by_text("green_trips", exact=True).wait_for(state="visible", timeout=5_000)
    assert page.get_by_label("Query schema").count() == 0
    page.get_by_text("green_trips", exact=True).click()
    page.locator(".column-item").get_by_text("trip_distance", exact=True).wait_for()
    assert page.locator(".column-item").get_by_text("Float64", exact=True).is_visible()
    editor = page.locator(".monaco-editor")
    editor.click(position={"x": 50, "y": 20})
    page.keyboard.insert_text("SELECT * FROM nyc_taxi.green_trips LIMIT 1")
    page.get_by_title("Run query (Ctrl+Enter)").click()
    page.locator("th").get_by_text("trip_distance", exact=True).wait_for(state="visible")
    assert page.get_by_text("42", exact=True).is_visible()
    assert not errors, errors
    page.screenshot(path=str(SCREENSHOT), full_page=True)
    browser.close()

print("PASS: Kaveon DB source -> OpenSource catalog -> schema -> table -> SQL Lab query result")
