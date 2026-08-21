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

# Kaveon brand palette — accent + complementary
C_ACCENT = "#4A9EE8"
C_ACCENT_DARK = "#2d7dd2"
C_TEAL = "#22d3ee"
C_EMERALD = "#10b981"
C_AMBER = "#f59e0b"
C_ROSE = "#f43f5e"
C_VIOLET = "#8b5cf6"
C_SLATE = "#64748b"


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


DISCLAIMER = "⚠️ _This is synthetic data generated to showcase the platform — not real usage._"


def text_box(content, x, y, w, h):
    return {"i": cell(), "type": "text", "textConfig": {"content": content},
            "x": x, "y": y, "w": w, "h": h, "minW": 2, "minH": 1, "maxW": 12, "maxH": 40}


def dash(name, desc, layout, filters=None):
    did = uuid.uuid4().hex
    cids = [b["chartId"] for b in layout if "chartId" in b]
    pool.execute_query(
        "INSERT INTO dashboards (id, name, description, layout, charts, filters, theme, "
        "is_published, visibility, created_by) VALUES (%s,%s,%s,%s,%s,%s,'dark',true,'published',%s)",
        DB, [did, name, desc, json.dumps(layout), json.dumps(cids),
             json.dumps(filters or []), OWNER])
    sys.stderr.write("Dashboard: %s (%d charts)\n" % (name, len(cids)))


def kpi(name, col, agg, label, filt=None):
    qc = {"query_mode": "aggregate", "metrics": [{"column": col, "aggregate": agg, "label": label}]}
    if filt:
        qc["filters"] = filt
    return chart(name, "big_number", qc)


def bar(name, col, agg, label, groupby, limit=None, horizontal=False, color=C_ACCENT, sort_desc=True):
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


DEFAULT_FILTERS = [
    {"id": "f-segment", "column": "segment", "label": "Segment",     "operator": "=", "value": "AllUp",
     "enabled": True, "appliesTo": "all", "datasetId": DS},
    {"id": "f-sub",     "column": "sub_segment", "label": "Sub-Segment", "operator": "=", "value": "AllUp",
     "enabled": True, "appliesTo": "all", "datasetId": DS},
    {"id": "f-industry","column": "industry","label": "Industry",    "operator": "=", "value": "AllUp",
     "enabled": True, "appliesTo": "all", "datasetId": DS},
    {"id": "f-plan",    "column": "plan",    "label": "Plan",         "operator": "=", "value": "AllUp",
     "enabled": True, "appliesTo": "all", "datasetId": DS},
    {"id": "f-region",  "column": "region",  "label": "Region",       "operator": "=", "value": "AllUp",
     "enabled": True, "appliesTo": "all", "datasetId": DS},
    {"id": "f-country", "column": "country", "label": "Country",      "operator": "=", "value": "AllUp",
     "enabled": True, "appliesTo": "all", "datasetId": DS},
    {"id": "f-org",     "column": "org",     "label": "Organization", "operator": "=", "value": "AllUp",
     "enabled": True, "appliesTo": "all", "datasetId": DS},
    {"id": "f-role",    "column": "role",    "label": "Role",         "operator": "=", "value": "AllUp",
     "enabled": True, "appliesTo": "all", "datasetId": DS},
    {"id": "f-acq",     "column": "acquisition_channel", "label": "Channel", "operator": "=", "value": "AllUp",
     "enabled": True, "appliesTo": "all", "datasetId": DS},
    {"id": "f-date",    "column": "usage_date", "label": "Date Range", "operator": "=", "value": "",
     "filterType": "date_range", "dateFrom": "", "dateTo": "",
     "enabled": True, "appliesTo": "all", "datasetId": DS},
]


def main():
    pool.execute_query("SELECT 1", DB)
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
    a4 = kpi("Avg Active Minutes", "active_minutes", "AVG", "Min/User")

    a5 = chart("Daily Query Trend", "line",
               {"query_mode": "aggregate",
                "metrics": [{"column": "queries_run", "aggregate": "SUM", "label": "Queries"}],
                "groupby": ["usage_date"], "sort_by": {"column": "usage_date", "direction": "asc"}},
               {"echarts_option": {"color": [C_ACCENT]}})

    a6 = chart("Global Users by Country", "world_map",
               {"query_mode": "aggregate",
                "metrics": [{"column": "user_id", "aggregate": "COUNT_DISTINCT", "label": "Active Users"}],
                "groupby": ["country"]})

    a7 = bar("Queries by Segment", "queries_run", "SUM", "Queries", "segment", color=C_ACCENT_DARK)
    a8 = bar("Users by Industry", "user_id", "COUNT_DISTINCT", "Users", "industry",
             limit=10, horizontal=True, color=C_TEAL)

    a9 = chart("Engagement by Plan", "stacked_bar",
               {"query_mode": "aggregate",
                "metrics": [{"column": "nl_queries", "aggregate": "SUM", "label": "NL Queries"},
                            {"column": "sql_lab_runs", "aggregate": "SUM", "label": "SQL Lab"},
                            {"column": "dashboards_viewed", "aggregate": "SUM", "label": "Dashboards"},
                            {"column": "charts_created", "aggregate": "SUM", "label": "Charts"}],
                "groupby": ["plan"]},
               {"echarts_option": {"color": [C_ACCENT, C_TEAL, C_EMERALD, C_AMBER]}})

    a10 = bar("Users by Acquisition Channel", "user_id", "COUNT_DISTINCT", "Users",
              "acquisition_channel", color=C_VIOLET)

    dash("Kaveon Product Usage",
         "Kaveon platform usage across segments, industries, and geographies — "
         "queries, engagement, adoption by plan and channel.",
         [box(a1, 0, 0, 3, 4), box(a2, 3, 0, 3, 4), box(a3, 6, 0, 3, 4), box(a4, 9, 0, 3, 4),
          box(a5, 0, 4, 12, 7),
          box(a6, 0, 11, 12, 9),
          box(a7, 0, 20, 6, 7), box(a8, 6, 20, 6, 7),
          box(a9, 0, 27, 6, 7), box(a10, 6, 27, 6, 7),
          text_box(DISCLAIMER, 0, 34, 12, 2)],
         DEFAULT_FILTERS)

    # ── Dashboard 2: Adoption & Engagement ───────────────────────────────────
    b1 = kpi("Total NL Queries", "nl_queries", "SUM", "NL Queries")
    b2 = kpi("SQL Lab Runs", "sql_lab_runs", "SUM", "SQL Lab")
    b3 = kpi("Charts Created", "charts_created", "SUM", "Charts")
    b4 = kpi("Total Exports", "exports", "SUM", "Exports")

    b5 = bar("Top 10 Orgs by Queries", "queries_run", "SUM", "Queries", "org",
             limit=10, horizontal=True, color=C_AMBER)
    b6 = bar("Charts by Role", "charts_created", "SUM", "Charts", "role", color=C_ROSE)

    b7 = bar("Users by Sub-Segment", "user_id", "COUNT_DISTINCT", "Users", "sub_segment",
             limit=10, horizontal=True, color=C_EMERALD)
    b8 = bar("Avg Minutes by Segment", "active_minutes", "AVG", "Avg Min", "segment", color=C_ACCENT)

    b9 = chart("Daily Active Users Trend", "line",
               {"query_mode": "aggregate",
                "metrics": [{"column": "user_id", "aggregate": "COUNT_DISTINCT", "label": "DAU"}],
                "groupby": ["usage_date"], "sort_by": {"column": "usage_date", "direction": "asc"}},
               {"echarts_option": {"color": [C_EMERALD]}})

    dash("Kaveon Adoption & Engagement",
         "Adoption depth — NL vs SQL Lab usage, charts and exports, engagement by role and segment, "
         "top organizations.",
         [box(b1, 0, 0, 3, 4), box(b2, 3, 0, 3, 4), box(b3, 6, 0, 3, 4), box(b4, 9, 0, 3, 4),
          box(b5, 0, 4, 6, 8), box(b6, 6, 4, 6, 8),
          box(b7, 0, 12, 6, 7), box(b8, 6, 12, 6, 7),
          box(b9, 0, 19, 12, 7),
          text_box(DISCLAIMER, 0, 26, 12, 2)],
         DEFAULT_FILTERS)

    n = pool.execute_query(
        "SELECT COUNT(*) FROM dashboards WHERE name LIKE 'Kaveon %'", DB)["rows"][0][0]
    sys.stderr.write("done — %s Kaveon dashboards\n" % n)


if __name__ == "__main__":
    main()
