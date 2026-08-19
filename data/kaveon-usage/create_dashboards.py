"""Create Kaveon Product Usage dashboards over dataset 142 (kaveon_usage_daily).

Uses the live charts/dashboards schema (query_config + viz_config JSON, integer
chart ids, dashboards.charts = json array of ids). JSON is bound as parameters
(%s) so the metadata dialect layer doesn't mangle the [] in the JSON.

Run from apps/kaveon-api:  python ../../data/kaveon-usage/create_dashboards.py
"""
import os
import sys
import json
import uuid
import random
import string

_API = os.path.join(os.path.dirname(__file__), "..", "..", "apps", "kaveon-api")
sys.path.insert(0, os.path.abspath(_API))

import database.pool as pool  # noqa: E402

DB = "kaveon"
DS = 142
SRC = "kaveon.public.kaveon_usage_daily"
OWNER = "pruthvi.prodduturi@gmail.com"


def cell():
    return "u" + "".join(random.choices(string.hexdigits[:16], k=8))


def chart(name, ctype, qc, vc=None):
    qc = {"dataset_id": DS, "datasource": SRC, **qc}
    r = pool.execute_query(
        "INSERT INTO charts (name, chart_type, query_config, viz_config, visibility, created_by) "
        "VALUES (%s, %s, %s, %s, 'published', %s) RETURNING id",
        DB, [name, ctype, json.dumps(qc), json.dumps(vc or {}), OWNER])
    cid = (r.get("rows") or [[None]])[0][0]
    sys.stderr.write("  chart %s: %s\n" % (cid, name))
    return cid


def dash(name, desc, layout):
    did = uuid.uuid4().hex
    cids = [b["chartId"] for b in layout]
    pool.execute_query(
        "INSERT INTO dashboards (id, name, description, layout, charts, theme, "
        "is_published, visibility, created_by) VALUES (%s,%s,%s,%s,%s,'dark',true,'published',%s)",
        DB, [did, name, desc, json.dumps(layout), json.dumps(cids), OWNER])
    sys.stderr.write("Dashboard: %s (%d charts)\n" % (name, len(cids)))


def kpi(name, col, agg, label, filt=None):
    qc = {"query_mode": "aggregate", "metrics": [{"column": col, "aggregate": agg, "label": label}]}
    if filt:
        qc["filters"] = filt
    return chart(name, "big_number", qc)


def bar(name, col, agg, label, groupby, limit=None, horizontal=False, color="#6366f1", sort_desc=True):
    qc = {"query_mode": "aggregate",
          "metrics": [{"column": col, "aggregate": agg, "label": label}],
          "groupby": [groupby],
          "sort_by": {"column": col, "direction": "desc" if sort_desc else "asc"}}
    if limit:
        qc["row_limit"] = limit
    vc = {"echarts_option": {"color": [color]}}
    if horizontal:
        vc["chartTypeOptions"] = {"horizontal": True}
    return chart(name, "bar", qc, vc)


def main():
    pool.execute_query("SELECT 1", DB)
    # clear prior usage dashboards/charts (charts for ds 142 identified via query_config)
    pool.execute_query(
        "DELETE FROM dashboards WHERE name IN ('Kaveon Product Usage','Kaveon Adoption & Engagement')", DB)
    pool.execute_query("DELETE FROM charts WHERE query_config LIKE '%\"dataset_id\": 142%'", DB)

    box = lambda cid, x, y, w, h: {  # noqa: E731
        "i": cell(), "type": "chart", "chartId": cid, "x": x, "y": y, "w": w, "h": h,
        "minW": 2, "minH": 2, "maxW": 12, "maxH": 40}

    # ── Dashboard 1: Product Usage overview ──────────────────────────────────
    a1 = kpi("Total Queries", "queries_run", "SUM", "Queries")
    a2 = kpi("Active Users", "user_id", "COUNT_DISTINCT", "Users")
    a3 = kpi("Dashboard Views", "dashboards_viewed", "SUM", "Views")
    a4 = kpi("Avg Active Minutes", "active_minutes", "AVG", "Min")
    a5 = chart("Daily Query Trend", "line",
               {"query_mode": "aggregate",
                "metrics": [{"column": "queries_run", "aggregate": "SUM", "label": "Queries"}],
                "groupby": ["usage_date"], "sort_by": {"column": "usage_date", "direction": "asc"}},
               {"echarts_option": {"color": ["#6366f1"]}})
    a6 = bar("Queries by Plan", "queries_run", "SUM", "Queries", "plan", color="#8b5cf6")
    a7 = bar("Active Users by Region", "user_id", "COUNT_DISTINCT", "Users", "region", color="#22d3ee")
    a8 = chart("Feature Mix by Plan", "stacked_bar",
               {"query_mode": "aggregate",
                "metrics": [{"column": "nl_queries", "aggregate": "SUM", "label": "NL"},
                            {"column": "sql_lab_runs", "aggregate": "SUM", "label": "SQL Lab"},
                            {"column": "dashboards_viewed", "aggregate": "SUM", "label": "Dashboards"},
                            {"column": "charts_created", "aggregate": "SUM", "label": "Charts"}],
                "groupby": ["plan"]},
               {"echarts_option": {"color": ["#6366f1", "#22d3ee", "#8b5cf6", "#f59e0b"]}})
    dash("Kaveon Product Usage",
         "How the Kaveon platform is used — queries, dashboard views, active users and "
         "engagement by plan and region. Synthetic, daily 2026-01-01..current, 10M rows.",
         [box(a1, 0, 0, 3, 4), box(a2, 3, 0, 3, 4), box(a3, 6, 0, 3, 4), box(a4, 9, 0, 3, 4),
          box(a5, 0, 4, 12, 7),
          box(a6, 0, 11, 6, 7), box(a7, 6, 11, 6, 7),
          box(a8, 0, 18, 12, 7)])

    # ── Dashboard 2: Adoption & engagement ───────────────────────────────────
    b1 = kpi("Total NL Queries", "nl_queries", "SUM", "NL")
    b2 = kpi("SQL Lab Runs", "sql_lab_runs", "SUM", "Runs")
    b3 = kpi("Charts Created", "charts_created", "SUM", "Charts")
    b4 = kpi("Total Exports", "exports", "SUM", "Exports")
    b5 = bar("Top 10 Orgs by Dashboard Views", "dashboards_viewed", "SUM", "Views", "org",
             limit=10, horizontal=True, color="#f59e0b")
    b6 = bar("Charts Created by Role", "charts_created", "SUM", "Charts", "role", color="#ec4899")
    b7 = bar("Active Users by Plan", "user_id", "COUNT_DISTINCT", "Users", "plan", color="#10b981")
    b8 = bar("Avg Active Minutes by Role", "active_minutes", "AVG", "Min", "role", color="#3b82f6")
    dash("Kaveon Adoption & Engagement",
         "Adoption depth — NL vs SQL Lab usage, charts and exports, engagement by role, "
         "and the most active organizations. Synthetic.",
         [box(b1, 0, 0, 3, 4), box(b2, 3, 0, 3, 4), box(b3, 6, 0, 3, 4), box(b4, 9, 0, 3, 4),
          box(b5, 0, 4, 6, 8), box(b6, 6, 4, 6, 8),
          box(b7, 0, 12, 6, 7), box(b8, 6, 12, 6, 7)])

    n = pool.execute_query(
        "SELECT COUNT(*) FROM dashboards WHERE name LIKE 'Kaveon %'", DB)["rows"][0][0]
    sys.stderr.write("done — %s Kaveon dashboards\n" % n)


if __name__ == "__main__":
    main()
