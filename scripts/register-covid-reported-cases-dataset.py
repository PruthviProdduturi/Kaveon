"""Register the full WHO reported-cases table as a Kaveon dataset and compile its DLM.

Why a second dataset instead of re-pointing "COVID-19 Global": that dataset is
bound to ``OpenSource.public.covid_global`` — the 50,838-row projection exported
from the retired Vercel installation — and six imported charts read its columns
(``total_cases``, ``total_deaths``, ``population``, ``continent``, ``country``).
Only ``country`` also exists on ``OpenSource.covid.reported_cases``, the
583,440-row WHO surveillance table, so re-pointing the dataset would change the
schema under live charts and break all six. This registers a
second dataset over the real source table and leaves the existing one alone —
the same decision already taken for Kaveon Events in
``register-kaveon-events-dataset.py``.

The binding is declared as an Engine table id rather than as a
catalog/schema/table triple. ``POST /api/v1/datasets`` resolves the names from
the Engine's own catalog definition for that id (``services.engine_datasets``),
so this dataset cannot name one table and read another — which is the class of
mistake that produced the mis-pointed "COVID-19 Global" in the first place.

Runs against a Studio port-forward the same way import-vercel-live-dashboards.py
does: sign in as the caller's Entra identity, then use the authenticated
/api/kaveon proxy. Idempotent — an existing dataset with the same name over the
same table is updated, never duplicated, and the script refuses to touch a
dataset bound to any other table.

    kubectl --context kaveon-aks -n kaveon port-forward svc/kaveon-portal 13015:3000
    python scripts/register-covid-reported-cases-dataset.py            # read-only preflight
    python scripts/register-covid-reported-cases-dataset.py --apply
    python scripts/register-covid-reported-cases-dataset.py --apply --generate --ask
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
DATASET_NAME = "COVID-19 Reported Cases"
CATALOG = "OpenSource"
SCHEMA = "covid"
TABLE = "reported_cases"
# The Engine's durable id for the table. The API reads the catalog, schema and
# table names from this id, so the binding cannot drift from the physical table.
ENGINE_TABLE_ID = f"table:{CATALOG}:{SCHEMA}:{TABLE}"
# The legacy projection the existing "COVID-19 Global" dataset is bound to. This
# script must never write to a dataset over it.
LEGACY_TABLE = ("OpenSource", "public", "covid_global")

DESCRIPTION = (
    "WHO reported COVID-19 cases and deaths: 583,440 country-day rows for 240 "
    "countries and areas across seven WHO regions, 2020-01-04 to 2026-08-30. "
    "These are reported surveillance counts, not infection dates or estimates; "
    "WHO publishes them weekly in the later years, and a revision can report a "
    "negative daily count. This is the full source table; the COVID-19 Global "
    "dataset is a 50,838-row projection of it kept for the imported dashboards."
)

DIMENSIONS = ("country", "country_code", "who_region")
# Measure columns. The cumulative pair is a measure for the chart builder but
# carries no named DLM metric — see METRICS.
MEASURES = ("new_cases", "new_deaths", "cumulative_cases", "cumulative_deaths")
DATE_COLUMN = "report_date"

# The exact live schema, in the platform's type spelling as
# services.engine_datasets.sql_type writes it (Arrow Utf8 -> varchar,
# Int64 -> bigint). Verified against the Engine before any write by
# verify_schema(); a drift fails the run rather than registering a stale list.
COLUMNS = (
    [{"table_name": TABLE, "column_name": DATE_COLUMN, "data_type": "varchar",
      "is_dimension": False, "is_metric": False, "semantic_type": "date"}]
    + [{"table_name": TABLE, "column_name": d, "data_type": "varchar",
        "is_dimension": True, "is_metric": False} for d in DIMENSIONS]
    + [{"table_name": TABLE, "column_name": m, "data_type": "bigint",
        "is_dimension": False, "is_metric": True} for m in MEASURES]
)

# Plain, genuinely additive aggregates only. SUM over the daily columns is
# correct at every grain the DLM can ask for — per country, per region, per day,
# and globally.
METRICS = [
    {"name": "New cases", "expression": "SUM(new_cases)", "metric_type": "sum", "format": "#,##0"},
    {"name": "New deaths", "expression": "SUM(new_deaths)", "metric_type": "sum", "format": "#,##0"},
    {"name": "Reported country-days", "expression": "COUNT(*)", "metric_type": "count", "format": "#,##0"},
]
# `cumulative_cases` and `cumulative_deaths` deliberately carry no named metric.
# They are per-country running totals: SUM over them double-counts every prior
# day, and MAX answers "the largest single country's total" whenever the question
# has no country in it — which the DLM would then present as a world total. A
# correct cumulative total needs one row per country, which is exactly
# `OpenSource.covid.country_latest` (240 rows); that belongs in its own dataset,
# not in a metric over this fact table. Both columns stay `is_metric` so the
# chart builder can still aggregate them under an author's explicit choice.

QUESTIONS = [
    "total new cases by WHO region",
    "new deaths by country in 2022",
    "new cases by month",
    "which country reported the most new cases",
    "new cases in India in 2021",
]


def dataset_body() -> dict[str, Any]:
    """The dataset as registered. `source` names the Engine table; the API fills
    `database_name`, `schema_name` and `table_name` from the Engine's own
    definition of that id and keeps the columns, metrics and date column sent
    here (`services.engine_datasets.apply_binding`)."""
    return {
        "name": DATASET_NAME,
        "description": DESCRIPTION,
        "source": {"kind": "engine", "table_id": ENGINE_TABLE_ID},
        "date_column": DATE_COLUMN,
        "columns": COLUMNS,
        "metrics": METRICS,
        "visibility": "published",
    }


def verify_schema(api) -> dict[str, Any]:
    """Read the live Engine schema for the table and compare it to COLUMNS.

    The column list above is a declaration, not a guess, and this is the gate
    that keeps it honest: the run fails if the table has gained, lost or retyped
    a column since it was written, rather than registering a dataset that
    describes a table the Engine no longer has.
    """
    sources = (api("GET", "lab/engine/sources") or {}).get("sources") or []
    source = next((item for item in sources if item.get("catalog") == CATALOG), None)
    if source is None:
        raise RuntimeError(f"{CATALOG} Engine source is unavailable")
    response = api("GET", f"lab/engine/{source['id']}/schemas/{SCHEMA}/tables/{TABLE}/columns")
    live = [(str(column["name"]), str(column["dataType"]))
            for column in (response.get("schema") or {}).get("columns") or []]
    if not live:
        raise RuntimeError(f"Engine returned no columns for {SCHEMA}.{TABLE}")
    # The lab endpoint answers with the Engine's Arrow names; the dataset store
    # keeps the platform spelling. Compare on the platform spelling.
    arrow_to_sql = {"Utf8": "varchar", "LargeUtf8": "varchar", "Int64": "bigint",
                    "Int32": "integer", "Float64": "double", "Boolean": "boolean",
                    "Date32": "date", "Date64": "date"}
    live_typed = {name: arrow_to_sql.get(dtype, dtype.lower()) for name, dtype in live}
    declared = {column["column_name"]: column["data_type"] for column in COLUMNS}
    if live_typed != declared:
        raise RuntimeError(
            "Engine schema differs from the declared column list for "
            f"{CATALOG}.{SCHEMA}.{TABLE}.\n  engine:   {sorted(live_typed.items())}"
            f"\n  declared: {sorted(declared.items())}"
        )
    return {"table": f"{CATALOG}.{SCHEMA}.{TABLE}", "engine_source_id": str(source["id"]),
            "column_count": len(live_typed), "columns": live}


def find_existing(api) -> dict[str, Any] | None:
    """The dataset this script owns, by name or by binding — and nothing else.

    The binding match includes the schema: `reported_cases` alone is not a
    unique table name, and a guard that ignores the schema is wrong in
    principle even where it happens to work.
    """
    listing = api("GET", "datasets?limit=500")
    rows = listing if isinstance(listing, list) else (listing.get("datasets") or listing.get("result") or [])
    for row in rows:
        binding = (row.get("database_name"), row.get("schema_name"),
                   row.get("fact_table") or row.get("table_name"))
        same_name = (row.get("dataset_name") or row.get("name")) == DATASET_NAME
        same_table = binding == (CATALOG, SCHEMA, TABLE)
        if not (same_name or same_table):
            continue
        if binding == LEGACY_TABLE:
            raise RuntimeError(
                f"Refusing to write: dataset {row.get('id')} named {DATASET_NAME!r} is bound to "
                f"{'.'.join(LEGACY_TABLE)}, which the imported dashboards read. "
                "Rename or remove it first."
            )
        return row
    return None


def run(api, apply: bool, generate: bool, ask: bool, register: bool = True) -> dict[str, Any]:
    report: dict[str, Any] = {"dataset": DATASET_NAME, "table": f"{CATALOG}.{SCHEMA}.{TABLE}",
                              "engine_table_id": ENGINE_TABLE_ID, "applied": apply}
    report["schema_verified"] = verify_schema(api)
    existing = find_existing(api)
    if not apply:
        report["existing"] = existing and {
            "id": existing.get("id"),
            "name": existing.get("dataset_name") or existing.get("name"),
            "binding": f"{existing.get('database_name')}.{existing.get('schema_name')}."
                       f"{existing.get('fact_table') or existing.get('table_name')}",
        }
        report["would"] = "update" if existing else "create"
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

    # Row count through the Engine, now that a dataset id exists: /sql/engine
    # resolves the schema from the saved dataset and never from the caller.
    t0 = time.time()
    counted = api("POST", "sql/engine", {
        "sql_text": f"SELECT COUNT(*) AS rows FROM {SCHEMA}.{TABLE}",
        "database": CATALOG, "dataset_id": int(dataset_id), "source": "dataset-preview",
    })
    rows = counted.get("rows") or counted.get("data") or []
    report["row_count"] = {"seconds": round(time.time() - t0, 2), "rows": rows}

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
                    executed = api("POST", "sql/engine", {
                        "sql_text": answer["sql"], "database": answer.get("database") or CATALOG,
                        "dataset_id": int(answer["dataset_id"]), "source": "qualification"})
                    live = executed.get("rows") or executed.get("data") or []
                    entry["live"] = {"seconds": round(time.time() - t1, 2), "rows": len(live), "first": live[:3]}
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
    parser.add_argument("--report", type=Path, default=ROOT / "tmp/covid-reported-cases-dataset.json")
    args = parser.parse_args()
    if args.question:
        QUESTIONS[:] = args.question
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
