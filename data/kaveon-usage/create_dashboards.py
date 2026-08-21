"""Create Kaveon Product Usage dashboards over the star schema VIEW.

Dataset 142 → kaveon.public.kaveon_product_analytics (joins L2 fact + all dims).

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
SRC = "kaveon.public.kaveon_product_analytics"
OWNER = "pruthvi.prodduturi@gmail.com"

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


DISCLAIMER = "This is synthetic data generated to showcase the platform."


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


def kpi(name, col, agg, label):
    qc = {"query_mode": "aggregate", "metrics": [{"column": col, "aggregate": agg, "label": label}]}
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
    {"id": "f-license", "column": "license", "label": "License",
     "operator": "=", "value": "AllUp", "enabled": True, "appliesTo": "all", "datasetId": DS},
    {"id": "f-audience", "column": "audience", "label": "Audience",
     "operator": "=", "value": "AllUp", "enabled": True, "appliesTo": "all", "datasetId": DS},
    {"id": "f-segment", "column": "segment", "label": "Segment",
     "operator": "=", "value": "AllUp", "enabled": True, "appliesTo": "all", "datasetId": DS},
    {"id": "f-industry", "column": "industry", "label": "Industry",
     "operator": "=", "value": "AllUp", "enabled": True, "appliesTo": "all", "datasetId": DS},
    {"id": "f-platform", "column": "platform", "label": "Platform",
     "operator": "=", "value": "AllUp", "enabled": True, "appliesTo": "all", "datasetId": DS},
    {"id": "f-os", "column": "os", "label": "OS",
     "operator": "=", "value": "AllUp", "enabled": True, "appliesTo": "all", "datasetId": DS},
    {"id": "f-region", "column": "region", "label": "Region",
     "operator": "=", "value": "AllUp", "enabled": True, "appliesTo": "all", "datasetId": DS},
    {"id": "f-country", "column": "country", "label": "Country",
     "operator": "=", "value": "AllUp", "enabled": True, "appliesTo": "all", "datasetId": DS},
    {"id": "f-team", "column": "team_size", "label": "Team Size",
     "operator": "=", "value": "AllUp", "enabled": True, "appliesTo": "all", "datasetId": DS},
    {"id": "f-deploy", "column": "deployment", "label": "Deployment",
     "operator": "=", "value": "AllUp", "enabled": True, "appliesTo": "all", "datasetId": DS},
    {"id": "f-acq", "column": "acquisition_channel", "label": "Channel",
     "operator": "=", "value": "AllUp", "enabled": True, "appliesTo": "all", "datasetId": DS},
    {"id": "f-date", "column": "usage_date", "label": "Date Range", "operator": "=", "value": "",
     "filterType": "date_range", "dateFrom": "", "dateTo": "",
     "enabled": True, "appliesTo": "all", "datasetId": DS},
]


def main():
    pool.execute_query("SELECT 1", DB)
    pool.execute_query(
        "DELETE FROM dashboards WHERE name IN "
        "('Kaveon Product Usage','Kaveon Adoption & Engagement','Kaveon Platform & Growth')", DB)
    pool.execute_query("DELETE FROM charts WHERE query_config LIKE '%\"dataset_id\": 142%'", DB)

    box = lambda cid, x, y, w, h: {  # noqa: E731
        "i": cell(), "type": "chart", "chartId": cid, "x": x, "y": y, "w": w, "h": h,
        "minW": 2, "minH": 2, "maxW": 12, "maxH": 40}

    # ── Dashboard 1: Product Usage ───────────────────────────────────────────
    a1 = kpi("Total Queries", "queries_run", "SUM", "Queries")
    a2 = kpi("Active Users", "user_id", "COUNT_DISTINCT", "Users")
    a3 = kpi("Dashboard Views", "dashboards_viewed", "SUM", "Views")
    a4 = kpi("Avg Active Minutes", "active_minutes", "AVG", "Min/User")

    a5 = chart("Queries by Platform", "bar",
               {"query_mode": "aggregate",
                "metrics": [{"column": "queries_run", "aggregate": "SUM", "label": "Queries"}],
                "groupby": ["platform"], "sort_by": {"column": "queries_run", "direction": "desc"}},
               {"echarts_option": {"color": [C_ACCENT]}})

    a6 = chart("Global Users by Country", "world_map",
               {"query_mode": "aggregate",
                "metrics": [{"column": "user_id", "aggregate": "COUNT_DISTINCT", "label": "Active Users"}],
                "groupby": ["country"]})

    a7 = bar("Queries by Segment", "queries_run", "SUM", "Queries", "segment", color=C_ACCENT_DARK)
    a8 = bar("Users by Industry", "user_id", "COUNT_DISTINCT", "Users", "industry",
             limit=10, horizontal=True, color=C_TEAL)

    a9 = chart("NL Queries by License", "bar",
               {"query_mode": "aggregate",
                "metrics": [{"column": "nl_queries", "aggregate": "SUM", "label": "NL Queries"}],
                "groupby": ["license"]},
               {"echarts_option": {"color": [C_ACCENT, C_TEAL, C_EMERALD, C_AMBER]}})

    a10 = bar("Users by Acquisition Channel", "user_id", "COUNT_DISTINCT", "Users",
              "acquisition_channel", color=C_VIOLET)

    dash("Kaveon Product Usage",
         "Kaveon platform usage across segments, industries, and geographies.",
         [box(a1, 0, 0, 3, 4), box(a2, 3, 0, 3, 4), box(a3, 6, 0, 3, 4), box(a4, 9, 0, 3, 4),
          box(a5, 0, 4, 12, 7),
          box(a6, 0, 11, 12, 9),
          box(a7, 0, 20, 6, 7), box(a8, 6, 20, 6, 7),
          box(a9, 0, 27, 6, 7), box(a10, 6, 27, 6, 7),
          text_box(DISCLAIMER, 0, 34, 12, 2)],
         DEFAULT_FILTERS)

    # ── Dashboard 2: Adoption & Engagement ───────────────────────────────────
    b1 = kpi("Total NL Queries", "nl_queries", "SUM", "NL Queries")
    b2 = kpi("Total Sessions", "sessions", "SUM", "Sessions")
    b3 = kpi("API Calls", "api_calls", "SUM", "API Calls")
    b4 = kpi("Data Processed (MB)", "data_processed_mb", "SUM", "MB Processed")

    b5 = chart("Active Users by Segment", "bar",
               {"query_mode": "aggregate",
                "metrics": [{"column": "user_id", "aggregate": "COUNT_DISTINCT", "label": "Active Users"}],
                "groupby": ["segment"], "sort_by": {"column": "user_id", "direction": "desc"}},
               {"echarts_option": {"color": [C_EMERALD]}})

    b6 = bar("Sessions by License", "sessions", "SUM", "Sessions", "license", color=C_AMBER)
    b7 = bar("API Calls by Platform", "api_calls", "SUM", "API Calls", "platform", color=C_ROSE)
    b8 = bar("Avg Minutes by Segment", "active_minutes", "AVG", "Avg Min", "segment", color=C_ACCENT)

    b9 = chart("Positive Feedback by Audience", "bar",
               {"query_mode": "aggregate",
                "metrics": [{"column": "feedback_positive", "aggregate": "SUM", "label": "Positive Feedback"}],
                "groupby": ["audience"], "sort_by": {"column": "feedback_positive", "direction": "desc"}},
               {"echarts_option": {"color": [C_EMERALD]}})

    b10 = bar("Errors by OS", "errors", "SUM", "Errors", "os", color=C_SLATE)

    dash("Kaveon Adoption & Engagement",
         "Adoption depth — sessions, API calls, data processed, feedback, and error rates.",
         [box(b1, 0, 0, 3, 4), box(b2, 3, 0, 3, 4), box(b3, 6, 0, 3, 4), box(b4, 9, 0, 3, 4),
          box(b5, 0, 4, 12, 7),
          box(b6, 0, 11, 6, 7), box(b7, 6, 11, 6, 7),
          box(b8, 0, 18, 6, 7), box(b9, 6, 18, 6, 7),
          box(b10, 0, 25, 12, 7),
          text_box(DISCLAIMER, 0, 32, 12, 2)],
         DEFAULT_FILTERS)

    # ── Dashboard 3: Platform & Growth ───────────────────────────────────────
    c1 = kpi("Total Exports", "exports", "SUM", "Exports")
    c2 = kpi("Charts Created", "charts_created", "SUM", "Charts")
    c3 = kpi("Datasets Accessed", "datasets_accessed", "SUM", "Datasets")
    c4 = kpi("Total Errors", "errors", "SUM", "Errors")

    c5 = bar("Users by Region", "user_id", "COUNT_DISTINCT", "Users", "region", color=C_ACCENT)
    c6 = bar("Queries by Team Size", "queries_run", "SUM", "Queries", "team_size", color=C_TEAL)
    c7 = bar("Users by Deployment", "user_id", "COUNT_DISTINCT", "Users", "deployment", color=C_EMERALD)

    c8 = chart("Queries by Region Over Time", "stacked_bar",
               {"query_mode": "aggregate",
                "metrics": [{"column": "queries_run", "aggregate": "SUM", "label": "Queries"}],
                "groupby": ["region"],
                "sort_by": {"column": "queries_run", "direction": "desc"}},
               {"echarts_option": {"color": [C_ACCENT, C_TEAL, C_EMERALD, C_AMBER, C_ROSE, C_VIOLET]}})

    c9 = bar("Data Processed by License", "data_processed_mb", "SUM", "MB",
             "license", color=C_VIOLET)
    c10 = bar("Users by Locale", "user_id", "COUNT_DISTINCT", "Users", "locale",
              limit=15, horizontal=True, color=C_ACCENT_DARK)

    dash("Kaveon Platform & Growth",
         "Platform distribution, regional growth, team sizes, and deployment models.",
         [box(c1, 0, 0, 3, 4), box(c2, 3, 0, 3, 4), box(c3, 6, 0, 3, 4), box(c4, 9, 0, 3, 4),
          box(c5, 0, 4, 6, 7), box(c6, 6, 4, 6, 7),
          box(c7, 0, 11, 6, 7), box(c8, 6, 11, 6, 7),
          box(c9, 0, 18, 6, 7), box(c10, 6, 18, 6, 7),
          text_box(DISCLAIMER, 0, 25, 12, 2)],
         DEFAULT_FILTERS)

    n = pool.execute_query(
        "SELECT COUNT(*) FROM dashboards WHERE name LIKE 'Kaveon %'", DB)["rows"][0][0]
    sys.stderr.write("done — %s Kaveon dashboards\n" % n)


if __name__ == "__main__":
    main()
