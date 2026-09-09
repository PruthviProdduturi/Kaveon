"""Import the canonical eight Vercel dashboards into the OpenSource Engine catalog.

The contract is an API export, so chart names, query/viz configuration, dashboard
layout and filters are copied without reinterpretation.  Only legacy dataset IDs
and datasource strings are replaced with registered OpenSource physical datasets.

Objects are idempotent by their private ``_kaveon_live_ref`` marker.  Cleanup is
limited to previously managed dashboards and happens only after all eight saved
dashboards pass an exact structural verification.
"""

from __future__ import annotations

import argparse
import copy
import json
import os
from pathlib import Path
import subprocess
from typing import Any, Callable


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_CONTRACT = ROOT / "data/dashboard-templates/vercel-live-dashboard-contract.json"
MARKER = "_kaveon_live_ref"
EXPECTED_DASHBOARDS = 8
EXPECTED_CHARTS = 70

# Legacy Vercel dataset ID -> physical Kaveon Engine table.  Dataset 134 was a
# dangling metadata reference in Vercel; its physical table is reconstructed by
# curate-original-climate-dashboards.py and intentionally registered here.
PHYSICAL_DATASETS: dict[str, dict[str, str]] = {
    "132": {"name": "Global Energy", "schema_name": "climate_energy", "table_name": "energy_annual"},
    "133": {"name": "Global Temperature", "schema_name": "climate_energy", "table_name": "temperature_monthly"},
    "134": {"name": "Climate × Energy", "schema_name": "climate_energy", "table_name": "climate_x_energy"},
    "135": {"name": "AI Model Leaderboard", "schema_name": "ai_benchmarks", "table_name": "leaderboard"},
    "137": {"name": "AI Arena Battles", "schema_name": "ai_benchmarks", "table_name": "arena_battles"},
    "138": {"name": "AI Model Pricing", "schema_name": "ai_benchmarks", "table_name": "pricing"},
    "139": {"name": "COVID-19 Global", "schema_name": "public", "table_name": "covid_global"},
    "140": {"name": "NYC Taxi by Borough", "schema_name": "public", "table_name": "nyc_taxi_borough"},
    "144": {"name": "Kaveon Events", "schema_name": "public", "table_name": "kaveon_events_dashboard"},
}

EVENT_DATASET_ID = "144"
EVENT_DIMENSIONS = {
    "surface", "platform", "license", "segment", "industry", "region", "country",
    "deployment", "acquisition_channel", "team_size",
}
EVENT_METRICS = {
    ("SUM", "actions"), ("SUM", "sessions"), ("SUM", "queries_run"),
    ("SUM", "charts_created"), ("SUM", "errors"), ("SUM", "rows_scanned"),
    ("SUM", "cache_hits"), ("AVG", "latency_p75_ms"),
    ("COUNT_DISTINCT", "user_id"),
}


def validate_event_projection_contract(contract: dict[str, Any]) -> None:
    """Reject a future chart/filter that the lossless event projection cannot answer."""
    for chart in contract.get("charts", []):
        if str(chart.get("dataset_id")) != EVENT_DATASET_ID:
            continue
        query = decoded(chart.get("query_config") or chart.get("config") or {}, "event chart query")
        unknown_groups = set(query.get("groupby") or []) - EVENT_DIMENSIONS
        unknown_metrics = {
            (str(metric.get("aggregate", "")).upper(), str(metric.get("column", "")))
            for metric in query.get("metrics") or []
        } - EVENT_METRICS
        if unknown_groups or unknown_metrics:
            raise RuntimeError(
                f"Event chart {chart.get('id')} exceeds compact projection: "
                f"groups={sorted(unknown_groups)}, metrics={sorted(unknown_metrics)}"
            )
    for dashboard in contract.get("dashboards", []):
        for item in decoded(dashboard.get("filters"), f"filters for {dashboard.get('name')}"):
            if str(item.get("datasetId")) == EVENT_DATASET_ID and item.get("column") not in EVENT_DIMENSIONS:
                raise RuntimeError(f"Event filter {item.get('column')} exceeds compact projection")


def decoded(value: Any, label: str) -> Any:
    """Decode JSON fields returned by the legacy API while accepting native JSON."""
    if isinstance(value, str):
        try:
            return json.loads(value)
        except json.JSONDecodeError as exc:
            raise RuntimeError(f"Invalid serialized {label}") from exc
    return copy.deepcopy(value)


def validate_contract(contract: dict[str, Any]) -> None:
    charts = contract.get("charts")
    dashboards = contract.get("dashboards")
    # The exporter marks the whole payload incomplete when any dataset endpoint
    # is missing.  Dataset 134 is the known live dangling reference; its table
    # contract and reconstruction are local and explicit above.  No other gap is
    # accepted.
    if contract.get("complete") is not True and contract.get("missing_dataset_ids") != [134]:
        raise RuntimeError("Canonical contract has unsupported missing data")
    if not isinstance(charts, list) or not isinstance(dashboards, list):
        raise RuntimeError("Canonical contract must contain chart and dashboard lists")
    if len(charts) != EXPECTED_CHARTS or len(dashboards) != EXPECTED_DASHBOARDS:
        raise RuntimeError(f"Expected exact 8/70 contract, got {len(dashboards)}/{len(charts)}")
    chart_ids = {str(chart.get("id")) for chart in charts}
    if len(chart_ids) != EXPECTED_CHARTS or "None" in chart_ids:
        raise RuntimeError("Canonical chart IDs must be present and unique")
    referenced: set[str] = set()
    for dashboard in dashboards:
        ids = decoded(dashboard.get("charts"), f"charts for {dashboard.get('name')}")
        if not isinstance(ids, list):
            raise RuntimeError(f"Dashboard {dashboard.get('name')} has invalid charts")
        referenced.update(str(value) for value in ids)
        decoded(dashboard.get("layout"), f"layout for {dashboard.get('name')}")
        decoded(dashboard.get("filters"), f"filters for {dashboard.get('name')}")
    if referenced != chart_ids:
        raise RuntimeError("Dashboard chart references do not cover the exact 70-chart contract")
    dataset_ids = {str(chart.get("dataset_id")) for chart in charts}
    if dataset_ids != set(PHYSICAL_DATASETS):
        raise RuntimeError(f"Unexpected legacy dataset IDs: {sorted(dataset_ids)}")
    validate_event_projection_contract(contract)


def chart_body(chart: dict[str, Any], dataset_ids: dict[str, int]) -> dict[str, Any]:
    legacy_id = str(chart["dataset_id"])
    query = decoded(chart.get("query_config") or chart.get("config") or {}, "chart query_config")
    viz = decoded(chart.get("viz_config") or {}, "chart viz_config")
    query["dataset_id"] = dataset_ids[legacy_id]
    query.pop("datasource", None)
    query[MARKER] = f"vercel-chart:{chart['id']}"
    return {
        "name": chart["name"],
        "description": chart.get("description"),
        "dataset_id": dataset_ids[legacy_id],
        "chart_type": chart["chart_type"],
        "query_config": query,
        "viz_config": viz,
        "visibility": "published",
    }


def dashboard_body(
    dashboard: dict[str, Any], chart_ids: dict[str, str], dataset_ids: dict[str, int]
) -> dict[str, Any]:
    layout = decoded(dashboard.get("layout"), "dashboard layout")
    filters = decoded(dashboard.get("filters"), "dashboard filters")
    legacy_charts = decoded(dashboard.get("charts"), "dashboard charts")
    if not isinstance(layout, list) or not isinstance(filters, list) or not isinstance(legacy_charts, list):
        raise RuntimeError(f"Dashboard {dashboard['name']} has malformed structure")
    marker = f"vercel-dashboard:{dashboard['id']}"
    if not layout:
        raise RuntimeError(f"Dashboard {dashboard['name']} has an empty layout")
    # An inert private property preserves the exact item list and geometry.
    layout[0][MARKER] = marker
    for item in layout:
        if item.get("type") == "chart":
            legacy_chart = str(item.get("chartId"))
            if legacy_chart not in chart_ids:
                raise RuntimeError(f"Unknown chart {legacy_chart} in {dashboard['name']} layout")
            item["chartId"] = chart_ids[legacy_chart]
    for item in filters:
        legacy_dataset = str(item.get("datasetId"))
        if legacy_dataset not in dataset_ids:
            raise RuntimeError(f"Unknown dataset {legacy_dataset} in {dashboard['name']} filter")
        item["datasetId"] = dataset_ids[legacy_dataset]
    return {
        "name": dashboard["name"],
        "description": dashboard.get("description"),
        "theme": dashboard.get("theme") or "dark",
        "layout": layout,
        "charts": [chart_ids[str(value)] for value in legacy_charts],
        "filters": filters,
        "visibility": "published",
        "is_published": True,
    }


def object_marker(item: dict[str, Any], kind: str) -> str | None:
    if kind == "chart":
        query = decoded(item.get("query_config") or {}, "persisted chart config")
        return query.get(MARKER) if isinstance(query, dict) else None
    layout = decoded(item.get("layout") or [], "persisted dashboard layout")
    if not isinstance(layout, list):
        return None
    return next((entry.get(MARKER) for entry in layout if isinstance(entry, dict) and entry.get(MARKER)), None)


def find_managed(items: list[dict[str, Any]], marker: str, kind: str) -> dict[str, Any] | None:
    matches = [item for item in items if object_marker(item, kind) == marker]
    if len(matches) > 1:
        raise RuntimeError(f"Ambiguous managed {kind}: {marker}")
    return matches[0] if matches else None


def comparable_dashboard(body: dict[str, Any]) -> dict[str, Any]:
    result = {key: body.get(key) for key in ("name", "description", "theme", "visibility", "is_published")}
    result.update({key: decoded(body.get(key), key) for key in ("layout", "charts", "filters")})
    return result


def audit_contract(contract: dict[str, Any], api: Callable[..., Any]) -> dict[str, Any]:
    """Read-only preflight for the complete import and managed cleanup set."""
    validate_contract(contract)
    sources = api("GET", "lab/engine/sources").get("sources", [])
    source = next((item for item in sources if item.get("catalog") == "OpenSource"), None)
    if source is None:
        raise RuntimeError("OpenSource Engine source is unavailable")
    datasets = api("GET", "datasets")
    physical = []
    for legacy_id, target in PHYSICAL_DATASETS.items():
        matches = [row for row in datasets if row.get("database_name") == "OpenSource" and row.get("schema_name") == target["schema_name"] and row.get("table_name") == target["table_name"]]
        if len(matches) > 1:
            raise RuntimeError(f"Duplicate physical dataset: {target['schema_name']}.{target['table_name']}")
        try:
            metadata = api("GET", f"lab/engine/{source['id']}/schemas/{target['schema_name']}/tables/{target['table_name']}/columns")
            columns = metadata.get("schema", {}).get("columns", [])
            error = None if columns else "no columns"
        except RuntimeError as exc:
            columns, error = [], str(exc)
        physical.append({
            "legacy_dataset_id": legacy_id,
            "physical": f"OpenSource.{target['schema_name']}.{target['table_name']}",
            "engine_column_count": len(columns),
            "registered_dataset_id": str(matches[0]["id"]) if matches else None,
            "error": error,
        })
    existing_charts = api("GET", "charts")
    existing_dashboards = api("GET", "dashboards")
    expected_chart_markers = {f"vercel-chart:{item['id']}" for item in contract["charts"]}
    expected_dashboard_markers = {f"vercel-dashboard:{item['id']}" for item in contract["dashboards"]}
    managed_chart_markers = {marker for item in existing_charts if (marker := object_marker(item, "chart")) and marker.startswith("vercel-chart:")}
    managed_dashboard_markers = {marker for item in existing_dashboards if (marker := object_marker(item, "dashboard")) and marker.startswith("vercel-dashboard:")}
    stale = sorted(managed_dashboard_markers - expected_dashboard_markers)
    return {
        "mode": "read-only",
        "contract": {"dashboard_count": len(contract["dashboards"]), "chart_count": len(contract["charts"]), "structurally_exact": True},
        "physical_datasets": physical,
        "all_engine_tables_available": all(not row["error"] for row in physical),
        "registered_dataset_count": sum(row["registered_dataset_id"] is not None for row in physical),
        "managed_charts_present": len(managed_chart_markers & expected_chart_markers),
        "managed_dashboards_present": len(managed_dashboard_markers & expected_dashboard_markers),
        "stale_managed_dashboard_markers": stale,
        "cleanup_would_delete_count": len(stale),
        "ready_to_apply": all(not row["error"] for row in physical),
    }


def import_contract(contract: dict[str, Any], api: Callable[..., Any], apply: bool) -> dict[str, Any]:
    """Register data and upsert the exact contract through a small API callback."""
    validate_contract(contract)
    sources = api("GET", "lab/engine/sources").get("sources", [])
    source = next((item for item in sources if item.get("catalog") == "OpenSource"), None)
    if source is None:
        raise RuntimeError("OpenSource Engine source is unavailable")
    existing_datasets = api("GET", "datasets")
    dataset_ids: dict[str, int] = {}
    for legacy_id, physical in PHYSICAL_DATASETS.items():
        target = {"database_name": "OpenSource", "schema_name": physical["schema_name"], "table_name": physical["table_name"]}
        matches = [row for row in existing_datasets if all(row.get(key) == value for key, value in target.items())]
        if len(matches) > 1:
            raise RuntimeError(f"Duplicate physical dataset: {physical['schema_name']}.{physical['table_name']}")
        columns_response = api("GET", f"lab/engine/{source['id']}/schemas/{physical['schema_name']}/tables/{physical['table_name']}/columns")
        columns = columns_response.get("schema", {}).get("columns", [])
        if not columns:
            raise RuntimeError(f"Engine returned no columns for {physical['schema_name']}.{physical['table_name']}")
        body = {**target, "name": physical["name"], "description": "Managed exact Vercel dashboard source.", "columns": [
            {"table_name": physical["table_name"], "column_name": col["name"], "data_type": col["dataType"]}
            for col in columns
        ], "visibility": "published"}
        if matches:
            saved = api("PUT", f"datasets/{matches[0]['id']}", body) if apply else matches[0]
        elif apply:
            saved = api("POST", "datasets", body)
            existing_datasets.append(saved)
        else:
            raise RuntimeError(f"Dataset missing for {physical['schema_name']}.{physical['table_name']}; use --apply")
        dataset_ids[legacy_id] = int(saved["id"])

    charts = api("GET", "charts")
    chart_ids: dict[str, str] = {}
    for chart in contract["charts"]:
        marker = f"vercel-chart:{chart['id']}"
        body = chart_body(chart, dataset_ids)
        existing = find_managed(charts, marker, "chart")
        if apply:
            saved = api("PUT", f"charts/{existing['id']}", {k: v for k, v in body.items() if k != "visibility"}) if existing else api("POST", "charts", body)
            if existing is None:
                charts.append(saved)
        elif existing is None:
            raise RuntimeError(f"Managed chart missing: {chart['name']}; use --apply")
        else:
            saved = existing
        chart_ids[str(chart["id"])] = str(saved["id"])

    dashboards = api("GET", "dashboards")
    imported = []
    expected_markers = set()
    for dashboard in contract["dashboards"]:
        marker = f"vercel-dashboard:{dashboard['id']}"
        expected_markers.add(marker)
        body = dashboard_body(dashboard, chart_ids, dataset_ids)
        existing = find_managed(dashboards, marker, "dashboard")
        if apply:
            saved = api("PUT", f"dashboards/{existing['id']}", body) if existing else api("POST", "dashboards", body)
            saved = api("PUT", f"dashboards/{saved['id']}", {"visibility": "published", "is_published": True})
        elif existing is None:
            raise RuntimeError(f"Managed dashboard missing: {dashboard['name']}; use --apply")
        else:
            saved = existing
        verified = api("GET", f"dashboards/{saved['id']}")
        expected = comparable_dashboard(body)
        actual = comparable_dashboard(verified)
        if actual != expected:
            raise RuntimeError(f"Persisted dashboard differs from contract: {dashboard['name']}")
        imported.append({"id": str(saved["id"]), "name": saved["name"], "chart_count": len(body["charts"])})

    # Destructive cleanup is deliberately last and only touches our marker namespace.
    stale = [item for item in dashboards if (object_marker(item, "dashboard") or "").startswith("vercel-dashboard:") and object_marker(item, "dashboard") not in expected_markers]
    if apply:
        for item in stale:
            api("DELETE", f"dashboards/{item['id']}")
    return {"dashboards": imported, "dashboard_count": len(imported), "chart_count": len(chart_ids), "stale_managed_removed": len(stale) if apply else 0, "dataset_map": dataset_ids}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--portal", default="http://127.0.0.1:13015")
    parser.add_argument("--contract", type=Path, default=DEFAULT_CONTRACT)
    parser.add_argument("--report", type=Path, default=ROOT / "tmp/vercel-live-dashboard-import.json")
    parser.add_argument("--apply", action="store_true")
    args = parser.parse_args()
    contract = json.loads(args.contract.read_text(encoding="utf-8"))

    from playwright.sync_api import sync_playwright
    with sync_playwright() as playwright:
        request = playwright.request.new_context(base_url=args.portal.rstrip("/"), timeout=180_000)
        try:
            config = request.get("/api/auth/entra-config").json()
            auth = subprocess.run(["az.cmd" if os.name == "nt" else "az", "account", "get-access-token", "--tenant", config["tenantId"], "--scope", config["scope"], "-o", "json"], capture_output=True, text=True, check=True)
            token = json.loads(auth.stdout)["accessToken"]
            csrf = request.get("/api/auth/csrf").json()["csrfToken"]
            response = request.post("/api/auth/callback/entra-public", form={"csrfToken": csrf, "token": token, "callbackUrl": f"{args.portal.rstrip('/')}/dashboards"}, headers={"X-Auth-Return-Redirect": "1"})
            if not response.ok:
                raise RuntimeError("Portal sign-in failed")
            def api(method: str, path: str, body: dict[str, Any] | None = None) -> Any:
                result = request.fetch(f"/api/kaveon/api/v1/{path}", method=method, data=body)
                if not result.ok:
                    raise RuntimeError(f"{method} {path}: {result.status} {result.text()[:700]}")
                return None if result.status == 204 else result.json()
            report = import_contract(contract, api, True) if args.apply else audit_contract(contract, api)
            args.report.parent.mkdir(parents=True, exist_ok=True)
            args.report.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
            counts = report.get("contract", report)
            print(f"PASS: exact {counts['dashboard_count']}/{counts['chart_count']} Vercel contract verified")
        finally:
            request.dispose()


if __name__ == "__main__":
    main()
