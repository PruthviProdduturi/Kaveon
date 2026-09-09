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
        return route.fulfill(json={"success": True, "sources": [{"id": "source-1", "name": "ADLS medallion", "catalog": "kavedb"}]})
    if path == "/api/v1/lab/engine/source-1/schemas":
        return route.fulfill(json={"success": True, "schemas": ["bronze", "silver", "gold"]})
    if path.endswith("/schemas/bronze/tables") or path.endswith("/schemas/gold/tables"):
        return route.fulfill(json={"success": True, "tables": []})
    if path == "/api/v1/lab/engine/source-1/schemas/silver/tables":
        return route.fulfill(json={"success": True, "tables": ["orders"]})
    if path.endswith("/schemas/silver/tables/orders/columns"):
        return route.fulfill(json={"success": True, "schema": {"columns": [{"name": "order_id", "dataType": "Int64", "isNullable": False}]}})
    if path == "/api/v1/lab/query":
        body = request.post_data_json
        assert body["engineSourceId"] == "source-1"
        assert body["engineSchema"] == "silver"
        assert "database" not in body
        return route.fulfill(json={"success": True, "columns": ["order_id", "amount"], "rows": [[1, 42]], "rowCount": 1, "executionTime": 3})
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
    page.locator("select.sidebar-db-select").first.wait_for(state="visible", timeout=20_000)
    source = page.locator("select.sidebar-db-select").first
    page.wait_for_function("document.querySelector('select.sidebar-db-select')?.value === 'engine:source-1'")
    assert source.locator("option", has_text="No data sources").count() == 0
    for schema in ["bronze", "silver", "gold"]:
        page.locator(".schema-header").get_by_text(schema, exact=True).wait_for()
    page.locator("select.sidebar-db-select").nth(1).wait_for(state="visible")
    page.locator("select.sidebar-db-select").nth(1).select_option("silver")
    page.locator(".schema-header").get_by_text("silver", exact=True).click()
    page.get_by_text("orders", exact=True).wait_for(state="visible", timeout=5_000)
    page.get_by_text("orders", exact=True).click()
    page.locator(".column-item").get_by_text("order_id", exact=True).wait_for()
    assert page.locator(".column-item").get_by_text("Int64", exact=True).is_visible()
    editor = page.locator(".monaco-editor")
    editor.click(position={"x": 50, "y": 20})
    page.keyboard.insert_text("SELECT * FROM silver.orders LIMIT 1")
    page.get_by_title("Run query (Ctrl+Enter)").click()
    page.locator("th").get_by_text("order_id", exact=True).wait_for(state="visible")
    assert page.get_by_text("42", exact=True).is_visible()
    assert not errors, errors
    page.screenshot(path=str(SCREENSHOT), full_page=True)
    browser.close()

print("PASS: Engine source -> schema -> table -> SQL Lab query result")
