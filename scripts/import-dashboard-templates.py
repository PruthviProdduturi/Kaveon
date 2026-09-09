"""Validate and import the extracted rich product dashboard templates.

The importer deliberately only targets the physical Engine dataset
``OpenSource.kaveon_product.kaveon_product_analytics``.  With ``--apply`` it
registers or refreshes that dataset from the Engine's table-column endpoint;
without it, the script makes no mutations.

Before chart or dashboard writes it generates and executes every chart query
and fetches every dashboard filter's distinct values. Imported objects carry
an internal template reference in their persisted configuration or layout.
That reference, rather than a chart name (which is not unique), is the sole
idempotency key.
"""

from __future__ import annotations

import argparse
import copy
import json
import os
from pathlib import Path
import subprocess
from typing import Any
from urllib.parse import urlencode

from playwright.sync_api import sync_playwright


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_MANIFEST = ROOT / "data/dashboard-templates/original-dashboard-templates.json"
TARGET_DATASET = {
    "database_name": "OpenSource",
    "schema_name": "kaveon_product",
    "table_name": "kaveon_product_analytics",
}
PRODUCT_THEME = "dark"  # The original product script explicitly used this theme.


def is_numeric(data_type: str) -> bool:
    return any(token in data_type.casefold() for token in (
        "int", "decimal", "numeric", "double", "float", "real", "number",
    ))


def require_list(value: Any, label: str) -> list[dict[str, Any]]:
    if not isinstance(value, list):
        raise RuntimeError(f"Expected {label} to be a list, got {type(value).__name__}")
    return value


def find_one(items: list[dict[str, Any]], predicate, label: str) -> dict[str, Any]:
    matches = [item for item in items if predicate(item)]
    if len(matches) != 1:
        raise RuntimeError(f"Expected exactly one {label}; found {len(matches)}")
    return matches[0]


def find_marked(items: list[dict[str, Any]], logical_ref: str, kind: str) -> dict[str, Any] | None:
    if kind == "chart":
        matches = [item for item in items if item.get("query_config", {}).get("_kaveon_template_ref") == logical_ref]
    else:
        matches = []
        for item in items:
            try:
                layout = json.loads(item.get("layout") or "[]")
            except (TypeError, json.JSONDecodeError):
                layout = []
            if isinstance(layout, list) and any(
                isinstance(entry, dict) and entry.get("_kaveon_template_ref") == logical_ref
                for entry in layout
            ):
                matches.append(item)
    if len(matches) > 1:
        raise RuntimeError(f"Ambiguous managed {kind} for {logical_ref}")
    return matches[0] if matches else None


def remap_chart_config(chart: dict[str, Any], dataset_id: int) -> dict[str, Any]:
    config = copy.deepcopy(chart["query_config"])
    if config.get("dataset_ref") != "product/kaveon-product-analytics":
        raise RuntimeError(f"Unexpected dataset reference in {chart['logical_ref']}")
    config["dataset_id"] = dataset_id
    # The API derives the physical datasource from dataset_id.  Leaving the
    # historical kaveon.public value here would send generated SQL to the wrong
    # catalog, so it is import metadata rather than runtime configuration.
    config.pop("dataset_ref", None)
    config.pop("datasource", None)
    config["_kaveon_template_ref"] = chart["logical_ref"]
    return config


def remap_dashboard(
    dashboard: dict[str, Any], chart_ids: dict[str, str], dataset_id: int
) -> tuple[list[dict[str, Any]], list[str], list[dict[str, Any]]]:
    layout = copy.deepcopy(dashboard["layout"])
    if not layout:
        raise RuntimeError(f"Dashboard {dashboard['logical_ref']} has no layout")
    layout[0]["_kaveon_template_ref"] = dashboard["logical_ref"]
    for item in layout:
        logical_ref = item.pop("chart_ref", None)
        if logical_ref is not None:
            try:
                item["chartId"] = chart_ids[logical_ref]
            except KeyError as exc:
                raise RuntimeError(f"Unknown chart reference {logical_ref}") from exc

    chart_refs = dashboard["chart_refs"]
    try:
        chart_list = [chart_ids[logical_ref] for logical_ref in chart_refs]
    except KeyError as exc:
        raise RuntimeError(f"Unknown dashboard chart reference {exc.args[0]}") from exc

    filters = copy.deepcopy(dashboard["filters"])
    for filter_config in filters:
        if filter_config.pop("dataset_ref", None) != "product/kaveon-product-analytics":
            raise RuntimeError(f"Unexpected dataset reference in filter {filter_config.get('id')}")
        filter_config["datasetId"] = dataset_id
    return layout, chart_list, filters


def physical_dataset_body(raw_columns: list[dict[str, Any]]) -> dict[str, Any]:
    """Translate the Engine's authoritative column contract to dataset metadata."""
    if not raw_columns:
        raise RuntimeError("Engine returned no columns for kaveon_product_analytics")
    columns = []
    for column in raw_columns:
        name = column.get("name")
        data_type = column.get("dataType")
        if not isinstance(name, str) or not name or not isinstance(data_type, str) or not data_type:
            raise RuntimeError("Engine table metadata contains an invalid column")
        columns.append({
            "table_name": TARGET_DATASET["table_name"],
            "column_name": name,
            "data_type": data_type,
            "is_dimension": not is_numeric(data_type),
            "is_metric": is_numeric(data_type),
        })
    return {
        "name": "Kaveon Product Analytics",
        "description": "Physical OpenSource Engine dataset for the Kaveon product dashboards.",
        **TARGET_DATASET,
        "columns": columns,
        "visibility": "published",
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--portal", default="http://127.0.0.1:13009", help="Portal base URL")
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument(
        "--report",
        type=Path,
        default=ROOT / "tmp/dashboard-template-import-result.json",
        help="Write imported IDs for browser qualification after a successful --apply",
    )
    parser.add_argument("--apply", action="store_true", help="Create/update validated product objects")
    parser.add_argument(
        "--finalize-validated",
        action="store_true",
        help="Finalize dashboards only after a previously completed validation and strict chart comparison",
    )
    args = parser.parse_args()
    if args.finalize_validated and not args.apply:
        parser.error("--finalize-validated requires --apply")

    manifest = json.loads(args.manifest.read_text(encoding="utf-8"))
    product = manifest.get("templates", {}).get("product")
    if not isinstance(product, dict):
        raise RuntimeError("Manifest does not contain the product template family")
    charts = require_list(product.get("charts"), "product charts")
    dashboards = require_list(product.get("dashboards"), "product dashboards")
    if len(charts) != 30 or len(dashboards) != 3:
        raise RuntimeError(f"Expected the complete product family (30 charts/3 dashboards), got {len(charts)}/{len(dashboards)}")

    base = args.portal.rstrip("/")
    with sync_playwright() as playwright:
        request = playwright.request.new_context(base_url=base, timeout=180_000)
        try:
            config = request.get("/api/auth/entra-config").json()
            auth = subprocess.run(
                [
                    "az.cmd" if os.name == "nt" else "az",
                    "account",
                    "get-access-token",
                    "--tenant",
                    config["tenantId"],
                    "--scope",
                    config["scope"],
                    "-o",
                    "json",
                ],
                capture_output=True,
                text=True,
                check=True,
            )
            token = json.loads(auth.stdout)["accessToken"]
            csrf = request.get("/api/auth/csrf").json()["csrfToken"]
            response = request.post(
                "/api/auth/callback/entra-public",
                form={"csrfToken": csrf, "token": token, "callbackUrl": f"{base}/dashboards"},
                headers={"X-Auth-Return-Redirect": "1"},
            )
            if not response.ok:
                raise RuntimeError("Portal sign-in failed")
            session = request.get("/api/auth/session").json()
            if session.get("user", {}).get("role") != "Admin":
                raise RuntimeError("An authenticated Admin is required to import published templates")

            def api(method: str, path: str, body: dict[str, Any] | None = None) -> Any:
                response = request.fetch(f"/api/kaveon/api/v1/{path}", method=method, data=body)
                if not response.ok:
                    raise RuntimeError(f"{method} {path}: {response.status} {response.text()[:700]}")
                return response.json()

            sources = api("GET", "lab/engine/sources").get("sources", [])
            source = find_one(
                require_list(sources, "Engine sources"),
                lambda item: item.get("catalog") == "OpenSource",
                "OpenSource Engine source",
            )
            source_id = source["id"]
            tables = api("GET", f"lab/engine/{source_id}/schemas/kaveon_product/tables")
            if TARGET_DATASET["table_name"] not in require_list(tables.get("tables"), "kaveon_product tables"):
                raise RuntimeError("Engine does not expose kaveon_product.kaveon_product_analytics")
            table_metadata = api(
                "GET",
                f"lab/engine/{source_id}/schemas/kaveon_product/tables/kaveon_product_analytics/columns",
            )
            raw_columns = require_list(table_metadata.get("schema", {}).get("columns"), "Engine table columns")
            dataset_body = physical_dataset_body(raw_columns)

            datasets = require_list(api("GET", "datasets"), "datasets")
            matching_datasets = [
                item for item in datasets
                if all(item.get(key) == value for key, value in TARGET_DATASET.items())
            ]
            if len(matching_datasets) > 1:
                raise RuntimeError("Multiple physical product datasets already exist; refusing to choose one")
            if not matching_datasets:
                if not args.apply:
                    raise RuntimeError("Physical product dataset is absent; re-run with --apply to register it from Engine metadata")
                dataset = api("POST", "datasets", dataset_body)
                print(f"Registered physical dataset: {dataset['id']}", flush=True)
            else:
                dataset = matching_datasets[0]
                if args.apply:
                    dataset = api("PUT", f"datasets/{dataset['id']}", dataset_body)
                    print(f"Refreshed physical dataset columns: {dataset['id']}", flush=True)
            try:
                dataset_id = int(dataset["id"])
            except (KeyError, TypeError, ValueError) as exc:
                raise RuntimeError("The physical product dataset must have a numeric ID") from exc

            query_evidence: list[dict[str, Any]] = []
            filter_evidence: dict[str, dict[str, Any]] = {}
            existing_charts = require_list(api("GET", "charts"), "charts")
            existing_dashboards = require_list(api("GET", "dashboards"), "dashboards")
            chart_ids: dict[str, str] = {}
            if args.finalize_validated:
                for chart in charts:
                    existing = find_marked(existing_charts, chart["logical_ref"], "chart")
                    expected_config = remap_chart_config(chart, dataset_id)
                    if existing is None or existing.get("dataset_id") != str(dataset_id):
                        raise RuntimeError(f"Missing or mismatched managed chart {chart['logical_ref']}")
                    if existing.get("query_config") != expected_config or existing.get("viz_config") != chart["viz_config"]:
                        raise RuntimeError(f"Managed chart differs from manifest: {chart['logical_ref']}")
                    chart_ids[chart["logical_ref"]] = str(existing["id"])
                print("Strict manifest comparison passed for all 30 previously validated charts.", flush=True)
            else:
                # Validate every generated SQL query before chart/dashboard writes.
                for chart in charts:
                    config = remap_chart_config(chart, dataset_id)
                    generated = api("POST", "sql/generate", {"dataset_id": dataset_id, "chart_type": chart["chart_type"], "config": config})
                    sql_text = generated.get("sql_text")
                    if not isinstance(sql_text, str) or not sql_text.strip():
                        raise RuntimeError(f"SQL generation returned no query for {chart['logical_ref']}")
                    try:
                        result = api("POST", "sql/engine", {"sql_text": sql_text, "database": "OpenSource", "dataset_id": dataset_id, "source": "dashboard-template-import", "chart_type": chart["chart_type"]})
                    except RuntimeError as exc:
                        raise RuntimeError(f"Engine execution failed for {chart['logical_ref']} with generated SQL:\n{sql_text}") from exc
                    if not isinstance(result.get("columns"), list) or not isinstance(result.get("rows"), list) or not result["rows"]:
                        raise RuntimeError(f"Invalid or empty Engine response for {chart['logical_ref']}")
                    query_evidence.append({"logical_ref": chart["logical_ref"], "query_id": result.get("query_id") or result.get("queryId") or result.get("id"), "row_count": result.get("row_count", len(result["rows"]))})
                    print(f"Validated chart SQL: {chart['logical_ref']} ({len(result['rows'])} rows)", flush=True)
                validated_filters = 0
                for dashboard in dashboards:
                    for filter_config in dashboard["filters"]:
                        if filter_config.get("dataset_ref") != "product/kaveon-product-analytics":
                            raise RuntimeError(f"Unexpected dataset reference in {filter_config.get('id')}")
                        cache_key = filter_config["column"]
                        if cache_key not in filter_evidence:
                            query = urlencode({"dataset_id": dataset_id, "column": filter_config["column"], "limit": 100, "source": "dashboard-filter"})
                            values = api("GET", f"sql/distinct-filter-values?{query}")
                            if not values.get("success") or not isinstance(values.get("values"), list) or not values["values"]:
                                raise RuntimeError(f"Filter values unavailable for {dashboard['logical_ref']}:{filter_config['id']}")
                            filter_evidence[cache_key] = {"column": filter_config["column"], "value_count": len(values["values"])}
                        validated_filters += 1
                print(f"Validated {len(charts)} chart queries and {validated_filters} filter definitions ({len(filter_evidence)} distinct Engine requests).", flush=True)
                if not args.apply:
                    print("Validation succeeded; re-run with --apply to import the product dashboards.")
                    return
                for chart in charts:
                    logical_ref = chart["logical_ref"]
                    body = {"name": chart["name"], "description": chart.get("description"), "dataset_id": dataset_id, "chart_type": chart["chart_type"], "query_config": remap_chart_config(chart, dataset_id), "viz_config": chart["viz_config"], "visibility": "published"}
                    existing = find_marked(existing_charts, logical_ref, "chart")
                    if existing is None:
                        saved = api("POST", "charts", body)
                        existing_charts.append(saved)
                    else:
                        saved = api("PUT", f"charts/{existing['id']}", {key: value for key, value in body.items() if key != "visibility"})
                    chart_ids[logical_ref] = str(saved["id"])

            imported: list[dict[str, Any]] = []
            for dashboard in dashboards:
                logical_ref = dashboard["logical_ref"]
                layout, chart_list, filters = remap_dashboard(dashboard, chart_ids, dataset_id)
                body = {
                    "name": dashboard["name"],
                    "description": dashboard.get("description"),
                    "theme": PRODUCT_THEME,
                    "layout": layout,
                    "charts": chart_list,
                    "filters": filters,
                    "visibility": "published",
                    "is_published": True,
                }
                existing = find_marked(existing_dashboards, logical_ref, "dashboard")
                if existing is None:
                    saved = api("POST", "dashboards", body)
                    existing_dashboards.append(saved)
                else:
                    saved = api("PUT", f"dashboards/{existing['id']}", body)
                # DashboardCreate intentionally does not expose is_published;
                # create therefore needs this explicit state transition too.
                saved = api("PUT", f"dashboards/{saved['id']}", {
                    "visibility": "published",
                    "is_published": True,
                })
                verified = api("GET", f"dashboards/{saved['id']}")
                if not verified.get("is_published") or verified.get("visibility") != "published":
                    raise RuntimeError(f"Dashboard publication was not persisted: {logical_ref}")
                imported.append({
                    "id": str(saved["id"]),
                    "name": saved["name"],
                    "charts": chart_list,
                    "filters": [filter_config["id"] for filter_config in filters],
                })
                print(f"Imported dashboard: {saved['name']} ({saved['id']})", flush=True)
            args.report.parent.mkdir(parents=True, exist_ok=True)
            args.report.write_text(json.dumps({
                "dataset": {**TARGET_DATASET, "id": dataset_id},
                "validation_note": (
                    "Chart SQL and nonempty filter values were validated before the managed charts "
                    "were written; query IDs were not persisted after that interrupted run."
                    if args.finalize_validated else None
                ),
                "query_evidence": query_evidence,
                "filter_evidence": list(filter_evidence.values()),
                "dashboards": imported,
            }, indent=2) + "\n", encoding="utf-8")
            print(f"PASS: imported {len(dashboards)} dashboards and {len(charts)} charts.")
        finally:
            request.dispose()


if __name__ == "__main__":
    main()
