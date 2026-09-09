"""Qualify the exact eight-dashboard Vercel contract through API and browser.

This is read-only: it verifies the managed published inventory and saved
structure, executes all 70 chart definitions, and renders every dashboard.
Authentication and tokens remain in the Playwright context memory.
"""

import argparse
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import time

from playwright.sync_api import sync_playwright


ROOT = Path(__file__).resolve().parents[2]
IMPORT_SPEC = importlib.util.spec_from_file_location(
    "live_importer", ROOT / "scripts/import-vercel-live-dashboards.py"
)
live = importlib.util.module_from_spec(IMPORT_SPEC)
assert IMPORT_SPEC.loader is not None
IMPORT_SPEC.loader.exec_module(live)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--portal", default="http://127.0.0.1:3000")
    parser.add_argument("--contract", type=Path, default=live.DEFAULT_CONTRACT)
    parser.add_argument("--output", type=Path, default=ROOT / "tmp/vercel-live-dashboard-qualification.json")
    args = parser.parse_args()
    contract = json.loads(args.contract.read_text(encoding="utf-8"))
    live.validate_contract(contract)
    base = args.portal.rstrip("/")

    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(channel="msedge", headless=True)
        context = browser.new_context(viewport={"width": 1600, "height": 1200})
        config = context.request.get(base + "/api/auth/entra-config").json()
        auth = subprocess.run(
            ["az.cmd" if os.name == "nt" else "az", "account", "get-access-token",
             "--tenant", config["tenantId"], "--scope", config["scope"], "-o", "json"],
            capture_output=True, text=True, check=True,
        )
        csrf = context.request.get(base + "/api/auth/csrf").json()["csrfToken"]
        signed_in = context.request.post(
            base + "/api/auth/callback/entra-public",
            form={"csrfToken": csrf, "token": json.loads(auth.stdout)["accessToken"], "callbackUrl": base + "/dashboards"},
            headers={"X-Auth-Return-Redirect": "1"},
        )
        if not signed_in.ok:
            raise RuntimeError("Portal sign-in failed")
        if context.request.get(base + "/api/auth/session").json().get("user", {}).get("role") != "Admin":
            raise RuntimeError("An authenticated Admin is required for complete qualification")

        def api(method: str, path: str, body=None):
            response = context.request.fetch(base + "/api/kaveon/api/v1/" + path, method=method, data=body)
            if not response.ok:
                raise RuntimeError(f"{method} {path}: {response.status} {response.text()[:700]}")
            return response.json()

        datasets = api("GET", "datasets")
        dataset_ids = {}
        for legacy_id, physical in live.PHYSICAL_DATASETS.items():
            matches = [item for item in datasets if item.get("database_name") == "OpenSource"
                       and item.get("schema_name") == physical["schema_name"]
                       and item.get("table_name") == physical["table_name"]]
            if len(matches) != 1:
                raise RuntimeError(f"Expected one registered dataset for OpenSource.{physical['schema_name']}.{physical['table_name']}; found {len(matches)}")
            dataset_ids[legacy_id] = int(matches[0]["id"])

        all_charts = api("GET", "charts")
        expected_chart_markers = {f"vercel-chart:{chart['id']}" for chart in contract["charts"]}
        managed_charts = [chart for chart in all_charts if (live.object_marker(chart, "chart") or "").startswith("vercel-chart:")]
        managed_chart_markers = {live.object_marker(chart, "chart") for chart in managed_charts}
        if len(managed_charts) != 70 or managed_chart_markers != expected_chart_markers:
            raise RuntimeError(f"Managed chart inventory differs: {len(managed_charts)}/70")
        charts_by_marker = {live.object_marker(chart, "chart"): chart for chart in managed_charts}
        chart_ids = {str(source["id"]): str(charts_by_marker[f"vercel-chart:{source['id']}"]["id"]) for source in contract["charts"]}

        all_dashboards = api("GET", "dashboards")
        expected_dashboard_markers = {f"vercel-dashboard:{item['id']}" for item in contract["dashboards"]}
        managed_dashboards = [item for item in all_dashboards if (live.object_marker(item, "dashboard") or "").startswith("vercel-dashboard:")]
        managed_dashboard_markers = {live.object_marker(item, "dashboard") for item in managed_dashboards}
        if len(managed_dashboards) != 8 or managed_dashboard_markers != expected_dashboard_markers:
            raise RuntimeError(f"Managed dashboard inventory differs: {len(managed_dashboards)}/8")

        dashboards_by_marker = {live.object_marker(item, "dashboard"): item for item in managed_dashboards}
        qualified_dashboards = []
        referenced_chart_ids = set()
        for source in contract["dashboards"]:
            marker = f"vercel-dashboard:{source['id']}"
            listed = dashboards_by_marker[marker]
            saved = api("GET", f"dashboards/{listed['id']}")
            expected = live.comparable_dashboard(live.dashboard_body(source, chart_ids, dataset_ids))
            if live.comparable_dashboard(saved) != expected:
                raise RuntimeError(f"Saved dashboard structure differs: {source['name']}")
            if saved.get("visibility") != "published" or saved.get("is_published") is not True:
                raise RuntimeError(f"Dashboard is not published: {source['name']}")
            referenced_chart_ids.update(str(value) for value in live.decoded(saved["charts"], "saved charts"))
            qualified_dashboards.append({"id": str(saved["id"]), "name": saved["name"], "chart_count": len(expected["charts"])})
        if referenced_chart_ids != set(chart_ids.values()):
            raise RuntimeError("Eight dashboards do not reference exactly the 70 managed charts")

        executions = []
        for source in contract["charts"]:
            chart = charts_by_marker[f"vercel-chart:{source['id']}"]
            expected = live.chart_body(source, dataset_ids)
            if chart.get("name") != expected["name"] or chart.get("chart_type") != expected["chart_type"]:
                raise RuntimeError(f"Saved chart identity differs: {source['name']}")
            if live.decoded(chart.get("query_config"), "saved query config") != expected["query_config"]:
                raise RuntimeError(f"Saved query configuration differs: {source['name']}")
            if live.decoded(chart.get("viz_config"), "saved viz config") != expected["viz_config"]:
                raise RuntimeError(f"Saved visualization differs: {source['name']}")
            generated = api("POST", "sql/generate", {"dataset_id": expected["dataset_id"], "chart_type": expected["chart_type"], "config": expected["query_config"]})
            sql = generated.get("sql_text")
            if not isinstance(sql, str) or not sql.strip():
                raise RuntimeError(f"No generated SQL for {source['name']}")
            try:
                result = api("POST", "sql/engine", {"sql_text": sql, "database": "OpenSource", "dataset_id": expected["dataset_id"], "chart_type": expected["chart_type"], "source": "vercel-live-dashboard-qualification", "chart_id": str(chart["id"])})
            except RuntimeError as exc:
                raise RuntimeError(f"Chart execution failed for {source['name']}: {sql}\n{exc}") from exc
            if not result.get("query_id") or not result.get("rows"):
                raise RuntimeError(f"Empty or untraced Engine result for {source['name']}")
            executions.append({"chart_id": str(chart["id"]), "name": source["name"], "query_id": result["query_id"], "row_count": len(result["rows"])})
            print(f"Executed {len(executions)}/70: {source['name']}", flush=True)

        renders = []
        for dashboard in qualified_dashboards:
            page = context.new_page()
            browser_errors, api_failures = [], []
            page.on("pageerror", lambda error: browser_errors.append(str(error)))
            page.on("response", lambda response: api_failures.append({"url": response.url, "status": response.status}) if "/api/v1/" in response.url and response.status >= 400 else None)
            page.goto(base + f"/dashboards/{dashboard['id']}/view", wait_until="domcontentloaded")
            deadline = time.monotonic() + 300
            while time.monotonic() < deadline:
                if browser_errors or api_failures:
                    break
                if page.locator(".dashboard-chart-component").count() == dashboard["chart_count"] and not page.get_by_text("Loading chart", exact=False).count() and not page.get_by_text("Querying", exact=False).count():
                    page.wait_for_timeout(1200)
                    break
                page.wait_for_timeout(500)
            if browser_errors or api_failures:
                raise RuntimeError({"dashboard": dashboard["name"], "browser_errors": browser_errors, "api_failures": api_failures})
            rendered = page.locator(".dashboard-chart-component").count()
            if rendered != dashboard["chart_count"]:
                raise RuntimeError(f"Rendered {rendered}/{dashboard['chart_count']} charts for {dashboard['name']}")
            body = page.locator("body").inner_text()
            if "Failed to load chart" in body or "Configure your chart to see preview" in body:
                raise RuntimeError(f"Chart error state rendered in {dashboard['name']}")
            screenshot = ROOT / "tmp" / f"vercel-live-{dashboard['id']}.png"
            page.screenshot(path=str(screenshot), full_page=True)
            renders.append({**dashboard, "rendered_chart_count": rendered, "screenshot": str(screenshot)})
            print(f"Rendered: {dashboard['name']} ({rendered} charts)", flush=True)
            page.close()

        report = {
            "status": "passed", "managed_dashboard_count": 8, "managed_chart_count": 70,
            "published": True, "exact_structure": True, "executions": executions, "renders": renders,
        }
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        browser.close()
        print("PASS: exact 8 dashboards/70 charts published, executed, and rendered", flush=True)


if __name__ == "__main__":
    main()
