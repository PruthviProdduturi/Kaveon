"""Build demo datasets + charts + dashboards on the Neon demo DB.

Inserts metadata directly (correct Postgres types) and verifies every chart's
SQL via the running lens-api (/sql/generate -> /sql/execute) so no broken tiles
ship. Idempotent-ish: clears prior demo datasets/charts/dashboards first.

Env: NEON_URL, LENS_PROXY_SECRET (defaults to the local dev secret).
"""
import os
import re
import json
import uuid
import requests
import psycopg2

NEON = os.environ["NEON_URL"]
API = "http://localhost:8080"
EMAIL = "pruthvi.prodduturi@gmail.com"
SECRET = os.environ.get("LENS_PROXY_SECRET", "dev-only-proxy-secret-change-me-fedcba9876543210")
H = {"x-user-email": EMAIL, "x-user-role": "Admin", "x-user-roles": "Admin",
     "x-proxy-secret": SECRET, "content-type": "application/json"}

conn = psycopg2.connect(NEON, connect_timeout=30)
conn.autocommit = True
cur = conn.cursor()


def reset():
    cur.execute("DELETE FROM dashboards")
    cur.execute("DELETE FROM charts")
    cur.execute("DELETE FROM dataset_columns")
    cur.execute("DELETE FROM dataset_dimensions")
    cur.execute("DELETE FROM dataset_metrics")
    cur.execute("DELETE FROM datasets")
    cur.execute("DELETE FROM data_sources WHERE name = 'Neon Demo'")


def data_source():
    cur.execute("""INSERT INTO data_sources
        (name, type, connection_string, database_name, region, description, created_by, is_active)
        VALUES (%s,%s,%s,%s,'WW',%s,%s,true)""",
        ("Neon Demo", "PostgreSQL", NEON, "neondb", "Open-source Postgres demo (COVID + NYC taxi)", EMAIL))


def dataset(name, table, cols, date_col=None):
    cur.execute("""INSERT INTO datasets
        (dataset_name, description, fact_table, schema_name, database_name, date_column,
         tables_used, visibility, created_at, modified_at, created_by, modified_by)
        VALUES (%s,%s,%s,'public','neondb',%s,'{"filters":[]}','published',now(),now(),%s,%s)
        RETURNING id""", (name, name, table, date_col, EMAIL, EMAIL))
    did = cur.fetchone()[0]
    for cn, dt, is_dim, is_met in cols:
        cur.execute("""INSERT INTO dataset_columns
            (dataset_id, table_name, column_name, data_type, is_dimension, is_metric, semantic_type)
            VALUES (%s,%s,%s,%s,%s,%s,NULL)""", (did, table, cn, dt, is_dim, is_met))
    return did


def chart(name, dataset_id, chart_type, qc):
    """Verify the chart's SQL executes on Neon, then persist it."""
    g = requests.post(f"{API}/api/v1/sql/generate", headers=H,
                      json={"dataset_id": dataset_id, "chart_type": chart_type, "config": qc}, timeout=40)
    if not g.ok:
        print(f"  GEN FAIL  {name}: {g.status_code} {g.text[:160]}"); return None
    sql = g.json().get("sql_text")
    e = requests.post(f"{API}/api/v1/sql/execute", headers=H,
                      json={"sql_text": sql, "database": "neondb", "source": "chart-builder",
                            "dataset_id": dataset_id, "chart_type": chart_type}, timeout=90)
    if not e.ok:
        print(f"  EXEC FAIL {name}: {e.status_code} {e.text[:160]}\n    SQL: {sql[:200]}"); return None
    rows = e.json().get("rows", [])
    cur.execute("""INSERT INTO charts
        (name, description, chart_type, query_config, viz_config, visibility,
         created_on, changed_on, created_at, updated_at, created_by, updated_by)
        VALUES (%s,%s,%s,%s,'{}','published',now(),now(),now(),now(),%s,%s) RETURNING id""",
        (name, name, chart_type, json.dumps({**qc, "dataset_id": dataset_id}), EMAIL, EMAIL))
    cid = cur.fetchone()[0]
    print(f"  ok  [{chart_type}] {name}  ({len(rows)} rows)  id={cid}")
    return cid


def M(col, agg, label):
    return {"column": col, "aggregate": agg, "label": label}


def tile(chart_id, x, y, w, h):
    return {"i": "t" + uuid.uuid4().hex[:8], "type": "chart", "chartId": chart_id,
            "x": x, "y": y, "w": w, "h": h, "minW": 2, "minH": 2, "maxW": 12, "maxH": 24}


def dash(name, description, layout):
    slug = re.sub(r"[^a-z0-9-]", "", name.lower().replace(" ", "-"))
    chart_ids = [t["chartId"] for t in layout if t.get("chartId")]
    did = uuid.uuid4().hex
    cur.execute("""INSERT INTO dashboards
        (id, name, slug, description, layout, charts, filters, theme, tags, visibility,
         is_published, is_archived, created_by, modified_by, created_at, modified_at)
        VALUES (%s,%s,%s,%s,%s,%s,'[]',NULL,NULL,'published',true,false,%s,%s,now(),now())""",
        (did, name, slug, description, json.dumps(layout), json.dumps(chart_ids), EMAIL, EMAIL))
    print(f"  dashboard: {name}  id={did}  ({len(layout)} tiles)")
    return did


def main():
    print("reset…"); reset(); data_source()

    # ── Datasets ──────────────────────────────────────────────────────────────
    daily = dataset("COVID-19 Daily", "covid_daily", [
        ("country", "varchar", True, False), ("dt", "date", True, False),
        ("new_confirmed", "bigint", False, True), ("new_deaths", "bigint", False, True),
        ("confirmed", "bigint", False, True), ("deaths", "bigint", False, True),
    ], date_col="dt")
    country = dataset("COVID-19 by Country", "covid_country_latest", [
        ("country", "varchar", True, False),
        ("confirmed", "bigint", False, True), ("deaths", "bigint", False, True),
    ])
    print(f"datasets: daily={daily} country={country}")

    # ── COVID charts (chart_type = real builder template ids) ──────────────────
    print("COVID charts:")
    agg = "aggregate"
    ids = {}
    ids["kpi_cases"] = chart("Total Confirmed Cases", country, "big_number",
          {"metrics": [M("confirmed", "SUM", "Total Cases")], "query_mode": agg})
    ids["kpi_deaths"] = chart("Total Deaths", country, "big_number",
          {"metrics": [M("deaths", "SUM", "Total Deaths")], "query_mode": agg})
    ids["global_new"] = chart("Global New Cases Over Time", daily, "time_series_line",
          {"metrics": [M("new_confirmed", "SUM", "New Cases")], "groupby": ["dt"],
           "sort_by": {"column": "dt", "direction": "asc"}, "query_mode": agg, "row_limit": 2000})
    ids["global_cum"] = chart("Global Cumulative Cases", daily, "time_series_area",
          {"metrics": [M("confirmed", "SUM", "Total Cases")], "groupby": ["dt"],
           "sort_by": {"column": "dt", "direction": "asc"}, "query_mode": agg, "row_limit": 2000})
    ids["map"] = chart("Confirmed Cases by Country", country, "world_map",
          {"metrics": [M("confirmed", "SUM", "Cases")], "groupby": ["country"],
           "sort_by": {"column": "confirmed", "direction": "desc"}, "query_mode": agg, "row_limit": 250})
    ids["top_cases"] = chart("Top 15 Countries by Cases", country, "bar_horizontal",
          {"metrics": [M("confirmed", "SUM", "Total Cases")], "groupby": ["country"],
           "sort_by": {"column": "confirmed", "direction": "desc"}, "query_mode": agg, "row_limit": 15})
    ids["top_deaths"] = chart("Top 15 Countries by Deaths", country, "bar_horizontal",
          {"metrics": [M("deaths", "SUM", "Total Deaths")], "groupby": ["country"],
           "sort_by": {"column": "deaths", "direction": "desc"}, "query_mode": agg, "row_limit": 15})
    ids["share"] = chart("Deaths Share — Top 10", country, "donut",
          {"metrics": [M("deaths", "SUM", "Deaths")], "groupby": ["country"],
           "sort_by": {"column": "deaths", "direction": "desc"}, "query_mode": agg, "row_limit": 10})
    ids["us_new"] = chart("US Daily New Cases", daily, "time_series_line",
          {"metrics": [M("new_confirmed", "SUM", "New Cases")], "groupby": ["dt"],
           "filters": [{"column": "country", "op": "=", "value": "US"}],
           "sort_by": {"column": "dt", "direction": "asc"}, "query_mode": agg, "row_limit": 2000})
    ids["summary"] = chart("Country Summary", country, "table",
          {"metrics": [M("confirmed", "SUM", "Cases"), M("deaths", "SUM", "Deaths")], "groupby": ["country"],
           "sort_by": {"column": "confirmed", "direction": "desc"}, "query_mode": agg, "row_limit": 100})

    print("chart ids:", {k: v for k, v in ids.items() if v})

    # ── COVID dashboard ───────────────────────────────────────────────────────
    dash("COVID-19 Global Overview",
         "Global COVID-19 cases, deaths, trends and country breakdowns — live from Neon Postgres.",
         [
             tile(ids["kpi_cases"], 0, 0, 3, 3),
             tile(ids["kpi_deaths"], 3, 0, 3, 3),
             tile(ids["map"], 6, 0, 6, 7),
             tile(ids["global_new"], 0, 3, 6, 6),
             tile(ids["top_cases"], 6, 7, 6, 6),
             tile(ids["global_cum"], 0, 9, 6, 6),
             tile(ids["top_deaths"], 6, 13, 6, 6),
             tile(ids["share"], 0, 15, 6, 6),
             tile(ids["us_new"], 6, 19, 6, 6),
             tile(ids["summary"], 0, 21, 12, 7),
         ])
    print("done.")
    cur.close(); conn.close()


if __name__ == "__main__":
    main()
