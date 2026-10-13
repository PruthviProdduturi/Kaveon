"""Register the product-telemetry table as a Kaveon dataset and compile its DLM.

Runs against a Studio port-forward the same way import-vercel-live-dashboards.py
does: sign in as the caller's Entra identity, then use the authenticated
/api/kaveon proxy. Idempotent: an existing dataset with the same name over the
same table is updated, never duplicated.

    kubectl --context kaveon-aks -n kaveon port-forward svc/kaveon-portal 13015:3000
    python scripts/register-kaveon-events-dataset.py                 # reads only
    python scripts/register-kaveon-events-dataset.py --apply
    python scripts/register-kaveon-events-dataset.py --apply --generate --ask

## The table this points at

`KaveonTelemetry.events.activity`, which is what
`scripts/build_kaveon_events_table.py` produces. The installation is currently
reading `KaveonTelemetry.events.enriched` - the cross-product build with
sequential integer identifiers - and the dataset that the charts bind to still
points there until the migration in
`docs/engineering/events-activity-rebuild.md` has been run. `--table` exists so
this script can address either one deliberately rather than by accident.

**Re-pointing a live dataset moves every chart that binds to it.** Run the
preflight, which writes nothing, and read what it reports before `--apply`.
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
# The dataset the installation already has, and the one its charts bind to.
# Registering a differently named dataset over the same table would leave two
# records describing one table, which is the shape of defects D-10 and D-20.
DATASET_NAME = "Kaveon Events"
CATALOG = "KaveonTelemetry"
SCHEMA = "events"
TABLE = "activity"
# No row count and no user count in the description. They are properties of
# whichever build is published, they change whenever the table is rebuilt, and
# a stale headline figure in a published description is how "504M rows"
# outlived the table it described.
DESCRIPTION = (
    "Kaveon product telemetry served by KaveonDB: one row per user, per "
    "surface, per day they were active across Chat, Dashboard, Chart Builder, "
    "SQL Lab, API and Export over July 2026 - actions, sessions, duration, "
    "queries, charts, errors, rows scanned, cache hits and latency, with "
    "platform, license, segment, industry, region, country, deployment, "
    "acquisition channel and team size. Deterministic demo data; not "
    "production telemetry."
)

DIMENSIONS = ("surface", "platform", "license", "segment", "industry", "region",
              "country", "deployment", "acquisition_channel", "team_size")
MEASURES = ("actions", "sessions", "duration_sec", "queries_run", "charts_created",
            "errors", "rows_scanned", "cache_hits", "latency_p75_ms")

# `user_id` is a UUID v4 held as text, not a bigint. Declaring it as a number
# would make every value fail to parse on its way into a filter, and the charts
# that count distinct users would be counting nothing.
COLUMNS = (
    [{"table_name": TABLE, "column_name": "event_date", "data_type": "date", "is_dimension": False, "is_metric": False, "semantic_type": "date"},
     {"table_name": TABLE, "column_name": "user_id", "data_type": "varchar", "is_dimension": False, "is_metric": False}]
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
# "Active users" = COUNT(DISTINCT user_id) is deliberately absent as a declared
# metric: exact distinct over a half-billion rows OOM-kills the 6 GiB Engine
# workers today (HANDSHAKE 2026-09-11, REQUEST @Codex), and a 36-character text
# key hashes more expensively than the int64 it replaces. The charts that count
# distinct users read the cube's HLL sketch instead, which is unaffected by
# either. Re-add the declared metric when exact distinct is fixed.

QUESTIONS = [
    "What is current Kaveon usage?",
    "total actions by surface",
    "active users by region",
    "queries run by license in July 2026",
    "errors by platform",
]


def rebind(table: str, dataset_name: str) -> None:
    """Point this registration at one table and one dataset name.

    Every entry in the column list carries `table_name`, so the table cannot be
    changed in one place only; this rewrites all of them from the one argument.
    """
    global TABLE, DATASET_NAME
    TABLE, DATASET_NAME = table, dataset_name
    for column in COLUMNS:
        column["table_name"] = table


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


def run(api, apply: bool, generate: bool, ask: bool, register: bool = True) -> dict[str, Any]:
    report: dict[str, Any] = {"dataset": DATASET_NAME, "table": f"{CATALOG}.{SCHEMA}.{TABLE}", "applied": apply}
    existing = find_existing(api)
    if not apply:
        # What the dataset is bound to *now*, so a re-point is a visible change
        # rather than a surprise: these charts follow whatever this says.
        report["existing"] = existing and {
            "id": existing.get("id"),
            "name": existing.get("dataset_name") or existing.get("name"),
            "bound_to": ".".join(str(existing.get(key) or "?") for key in
                                 ("database_name", "schema_name", "table_name")),
        }
        report["would_bind_to"] = f"{CATALOG}.{SCHEMA}.{TABLE}"
        return report
    if existing:
        dataset_id = str(existing["id"])
        if register:
            api("PUT", f"datasets/{dataset_id}", dataset_body())
        report["action"] = "updated" if register else "kept"
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
                     "dataset_id": answer.get("dataset_id"), "sql": answer.get("sql"),
                     **({"clarification": answer.get("clarification")} if answer.get("clarification") else {})}
            if answer.get("ok") and answer.get("sql") and not answer.get("from_context"):
                t1 = time.time()
                try:
                    if answer.get("engine"):
                        executed = api("POST", "sql/engine", {"sql_text": answer["sql"], "database": answer.get("database") or CATALOG,
                                                              "dataset_id": int(answer["dataset_id"]), "source": "qualification"})
                    else:
                        executed = api("POST", "sql/execute", {"sql_text": answer["sql"], "database": answer.get("database") or CATALOG, "source": "qualification"})
                    rows = executed.get("rows") or executed.get("data") or []
                    entry["live"] = {"seconds": round(time.time() - t1, 2), "rows": len(rows), "first": rows[:3]}
                except RuntimeError as exc:
                    entry["live"] = {"seconds": round(time.time() - t1, 2), "error": str(exc)[:300]}
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
    parser.add_argument("--no-register", action="store_true", help="with --apply: skip the dataset PUT/POST")
    parser.add_argument("--question", action="append", help="ask these instead of the built-in questions (repeatable)")
    parser.add_argument("--table", default=TABLE,
                        help=f"the table in {CATALOG}.{SCHEMA} to bind the dataset to "
                             f"(default {TABLE}); pass 'enriched' to address the "
                             "table published before the rebuild")
    parser.add_argument("--dataset-name", default=DATASET_NAME,
                        help=f"the dataset to create or update (default {DATASET_NAME!r})")
    parser.add_argument("--report", type=Path, default=ROOT / "tmp/kaveon-telemetry-dataset.json")
    args = parser.parse_args()
    if args.question:
        QUESTIONS[:] = args.question
    rebind(args.table, args.dataset_name)
    print(f"dataset {DATASET_NAME!r} -> {CATALOG}.{SCHEMA}.{TABLE}")
    from playwright.sync_api import sync_playwright
    with sync_playwright() as playwright:
        request = playwright.request.new_context(base_url=args.portal.rstrip("/"), timeout=1_800_000)
        try:
            config = request.get("/api/auth/entra-config").json()
            if config.get("enabled"):
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
            # Entra disabled: the local portal's development identity is already signed in.

            def api(method: str, path: str, body: dict[str, Any] | None = None) -> Any:
                result = request.fetch(f"/api/kaveon/api/v1/{path}", method=method, data=body)
                if not result.ok:
                    raise RuntimeError(f"{method} {path}: {result.status} {result.text()[:700]}")
                return None if result.status == 204 else result.json()

            report = run(api, args.apply, args.generate, args.ask, register=not args.no_register)
        finally:
            request.dispose()
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
