"""Register the 504M-row telemetry table as a Kaveon dataset and compile its DLM.

Runs against a Studio port-forward the same way import-vercel-live-dashboards.py
does: sign in as the caller's Entra identity, then use the authenticated
/api/kaveon proxy. Idempotent — an existing dataset with the same name over the
same table is updated, never duplicated. The existing "Kaveon Events" dataset
(the compact dashboard projection) is left alone; this is a second dataset.

    kubectl --context kaveon-test-aks -n kaveon port-forward svc/kaveon-portal 13015:3000
    python scripts/register-kaveon-events-dataset.py --apply
    python scripts/register-kaveon-events-dataset.py --apply --generate --ask
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
DATASET_NAME = "Kaveon Telemetry"
CATALOG = "OpenSource"
SCHEMA = "kaveon_product"
TABLE = "kaveon_events_enriched"
DESCRIPTION = (
    "504M-row product telemetry served by KaveonDB: daily per-user engagement "
    "across six surfaces (Chat, Dashboard, Chart Builder, SQL Lab, API, Export) "
    "for 3M users over July 2026 — actions, sessions, duration, queries, charts, "
    "errors, rows scanned, cache hits and latency, with platform, license, "
    "segment, industry, region, country, deployment, acquisition channel and "
    "team size. Deterministic demo data; not production telemetry."
)

DIMENSIONS = ("surface", "platform", "license", "segment", "industry", "region",
              "country", "deployment", "acquisition_channel", "team_size")
MEASURES = ("actions", "sessions", "duration_sec", "queries_run", "charts_created",
            "errors", "rows_scanned", "cache_hits", "latency_p75_ms")

COLUMNS = (
    [{"table_name": TABLE, "column_name": "event_date", "data_type": "varchar", "is_dimension": False, "is_metric": False, "semantic_type": "date"},
     {"table_name": TABLE, "column_name": "user_id", "data_type": "bigint", "is_dimension": False, "is_metric": False}]
    + [{"table_name": TABLE, "column_name": m, "data_type": "bigint", "is_dimension": False, "is_metric": True} for m in MEASURES]
    + [{"table_name": TABLE, "column_name": d, "data_type": "varchar", "is_dimension": True, "is_metric": False} for d in DIMENSIONS]
)

# Plain aggregates only: every expression here runs on the Engine and merges
# from the DLM's precomputed context. Ratios are left to charts.
METRICS = [
    {"name": "Total actions", "expression": "SUM(actions)", "metric_type": "sum", "format": "#,##0"},
    {"name": "Sessions", "expression": "SUM(sessions)", "metric_type": "sum", "format": "#,##0"},
    {"name": "Duration (sec)", "expression": "SUM(duration_sec)", "metric_type": "sum", "format": "#,##0"},
    {"name": "Queries run", "expression": "SUM(queries_run)", "metric_type": "sum", "format": "#,##0"},
    {"name": "Charts created", "expression": "SUM(charts_created)", "metric_type": "sum", "format": "#,##0"},
    {"name": "Errors", "expression": "SUM(errors)", "metric_type": "sum", "format": "#,##0"},
    {"name": "Rows scanned", "expression": "SUM(rows_scanned)", "metric_type": "sum", "format": "#,##0"},
    {"name": "Cache hits", "expression": "SUM(cache_hits)", "metric_type": "sum", "format": "#,##0"},
    {"name": "Average latency (ms)", "expression": "AVG(latency_p75_ms)", "metric_type": "avg", "format": "#,##0"},
]
# "Active users" = COUNT(DISTINCT user_id) is deliberately absent: exact distinct
# over 504M rows OOM-kills the 6 GiB Engine workers today (HANDSHAKE 2026-09-11,
# REQUEST @Codex). Re-add when that is fixed.

QUESTIONS = [
    "What is current Kaveon usage?",
    "total actions by surface",
    "active users by region",
    "queries run by license in July 2026",
    "errors by platform",
]


def dataset_body() -> dict[str, Any]:
    return {
        "name": DATASET_NAME,
        "description": DESCRIPTION,
        "table_name": TABLE,
        "schema_name": SCHEMA,
        "database_name": CATALOG,
        "date_column": "event_date",
        "columns": COLUMNS,
        "metrics": METRICS,
        "visibility": "published",
    }


def find_existing(api) -> dict[str, Any] | None:
    listing = api("GET", "datasets?limit=500")
    rows = listing if isinstance(listing, list) else (listing.get("datasets") or listing.get("result") or [])
    for row in rows:
        same_name = (row.get("dataset_name") or row.get("name")) == DATASET_NAME
        same_table = (row.get("fact_table") or row.get("table_name")) == TABLE and row.get("database_name") == CATALOG
        if same_name or same_table:
            return row
    return None


def run(api, apply: bool, generate: bool, ask: bool) -> dict[str, Any]:
    report: dict[str, Any] = {"dataset": DATASET_NAME, "table": f"{CATALOG}.{SCHEMA}.{TABLE}", "applied": apply}
    existing = find_existing(api)
    if not apply:
        report["existing"] = existing and {"id": existing.get("id"), "name": existing.get("dataset_name") or existing.get("name")}
        return report
    if existing:
        dataset_id = str(existing["id"])
        api("PUT", f"datasets/{dataset_id}", dataset_body())
        report["action"] = "updated"
    else:
        created = api("POST", "datasets", dataset_body())
        dataset_id = str(created["id"])
        report["action"] = "created"
    report["dataset_id"] = dataset_id

    if generate:
        t0 = time.time()
        result = api("POST", f"datasets/{dataset_id}/dlm/generate?force=true")
        report["generate"] = {
            "seconds": round(time.time() - t0, 1),
            "ok": result.get("ok"),
            "status": result.get("status"),
            "answers_precomputed": result.get("answers_precomputed"),
            "value_index_rows": result.get("value_index_rows") or result.get("values_indexed"),
        }
    if ask:
        asked = []
        for question in QUESTIONS:
            t0 = time.time()
            answer = api("POST", "dlm/ask", {"question": question})
            entry = {"question": question, "seconds": round(time.time() - t0, 2), "ok": answer.get("ok"),
                     "reason": answer.get("reason"), "from_context": answer.get("from_context"),
                     "dataset_id": answer.get("dataset_id"), "sql": answer.get("sql")}
            if answer.get("ok") and answer.get("sql") and not answer.get("from_context"):
                t1 = time.time()
                executed = api("POST", "sql/execute", {"sql_text": answer["sql"], "database": answer.get("database") or CATALOG, "source": "qualification"})
                rows = executed.get("rows") or executed.get("data") or []
                entry["live"] = {"seconds": round(time.time() - t1, 2), "rows": len(rows), "first": rows[:3]}
            elif answer.get("from_context"):
                entry["rows"] = (answer.get("rows") or [])[:3]
            asked.append(entry)
        report["asked"] = asked
    return report


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--portal", default="http://127.0.0.1:13015")
    parser.add_argument("--apply", action="store_true")
    parser.add_argument("--generate", action="store_true", help="compile the DLM after registering")
    parser.add_argument("--ask", action="store_true", help="ask the qualification questions and time them")
    parser.add_argument("--report", type=Path, default=ROOT / "tmp/kaveon-telemetry-dataset.json")
    args = parser.parse_args()
    from playwright.sync_api import sync_playwright
    with sync_playwright() as playwright:
        request = playwright.request.new_context(base_url=args.portal.rstrip("/"), timeout=1_800_000)
        try:
            config = request.get("/api/auth/entra-config").json()
            az = "az.cmd" if os.name == "nt" else "az"
            auth = subprocess.run([az, "account", "get-access-token", "--tenant", config["tenantId"],
                                   "--scope", config["scope"], "-o", "json"], capture_output=True, text=True, check=True)
            token = json.loads(auth.stdout)["accessToken"]
            csrf = request.get("/api/auth/csrf").json()["csrfToken"]
            response = request.post("/api/auth/callback/entra-public",
                                    form={"csrfToken": csrf, "token": token, "callbackUrl": f"{args.portal.rstrip('/')}/"},
                                    headers={"X-Auth-Return-Redirect": "1"})
            if not response.ok:
                raise RuntimeError("Portal sign-in failed")

            def api(method: str, path: str, body: dict[str, Any] | None = None) -> Any:
                result = request.fetch(f"/api/kaveon/api/v1/{path}", method=method, data=body)
                if not result.ok:
                    raise RuntimeError(f"{method} {path}: {result.status} {result.text()[:700]}")
                return None if result.status == 204 else result.json()

            report = run(api, args.apply, args.generate, args.ask)
        finally:
            request.dispose()
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
