"""Build demo datasets + charts + the COVID-19 Global Overview dashboard on Neon.

Inserts metadata directly (correct Postgres types) and verifies every chart's
SQL via the running lens-api (/sql/generate -> /sql/execute) so no broken tiles
ship. Idempotent-ish: clears prior demo datasets/charts/dashboards first.

The dashboard is designed to be genuinely interactive and presentation-grade:
  - 4 headline KPIs (population, cases, deaths, case-fatality-rate)
  - a full-bleed clickable world choropleth (click a country -> cross-filters all)
  - trend, ranking, %-affected, CFR, share and a detailed country table
  - markdown section blocks that narrate what each band of charts is showing
  - a global Country filter in the read-only filter bar

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


def chart(name, dataset_id, chart_type, qc, desc="", viz=None):
    """Verify the chart's SQL executes on Neon, then persist it.

    `desc` becomes the tile's info-tooltip subtitle; `viz` carries
    chartTypeOptions (number formats, suffixes) into the renderer.
    """
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
        VALUES (%s,%s,%s,%s,%s,'published',now(),now(),now(),now(),%s,%s) RETURNING id""",
        (name, desc or name, chart_type, json.dumps({**qc, "dataset_id": dataset_id}),
         json.dumps(viz or {}), EMAIL, EMAIL))
    cid = cur.fetchone()[0]
    print(f"  ok  [{chart_type}] {name}  ({len(rows)} rows)  id={cid}")
    return cid


def M(col, agg, label):
    return {"column": col, "aggregate": agg, "label": label}


# ── Layout primitives ────────────────────────────────────────────────────────

def _chart_child(cid, parent, w, h):
    return {"i": "c" + uuid.uuid4().hex[:8], "type": "chart", "chartId": cid, "parentId": parent,
            "x": 0, "y": 0, "w": w, "h": h, "minW": 2, "minH": 2, "maxW": 12, "maxH": 24}


def _row(chart_ids, height, y, widths=None):
    """A row container holding chart tiles side-by-side (widths sum to 12)."""
    rid = "row" + uuid.uuid4().hex[:8]
    n = len(chart_ids)
    if not widths:
        base = 12 // n
        widths = [base] * n
        widths[0] += 12 - base * n
    children = [_chart_child(cid, rid, widths[i], height) for i, cid in enumerate(chart_ids)]
    return {"i": rid, "type": "row", "x": 0, "y": y, "w": 12, "h": height,
            "minW": 12, "minH": 2, "maxW": 12, "maxH": 24, "children": children}


def _text(md, height, y, size=14, color="#475569"):
    return {"i": "txt" + uuid.uuid4().hex[:8], "type": "text", "x": 0, "y": y, "w": 12, "h": height,
            "minW": 2, "minH": 1, "maxW": 12, "maxH": 24,
            "textConfig": {"content": md, "alignment": "left", "fontSize": size, "color": color}}


def _country_filter(dataset_id):
    """A global Country filter (starts disabled → dashboard defaults to global)."""
    return {
        "id": "flt-country",
        "column": "covid_country_latest.country",
        "operator": "=",
        "value": "",
        "appliesTo": "all",
        "enabled": False,
        "datasetId": dataset_id,
        "filterType": "value",
    }


def dash(name, description, blocks, filters=None):
    """blocks: list of layout items already positioned (rows / text)."""
    layout, all_ids = [], []
    for b in blocks:
        layout.append(b)
        for ch in b.get("children", []):
            if ch.get("chartId"):
                all_ids.append(ch["chartId"])
    slug = re.sub(r"[^a-z0-9-]", "", name.lower().replace(" ", "-"))
    did = uuid.uuid4().hex
    cur.execute("""INSERT INTO dashboards
        (id, name, slug, description, layout, charts, filters, theme, tags, visibility,
         is_published, is_archived, created_by, modified_by, created_at, modified_at)
        VALUES (%s,%s,%s,%s,%s,%s,%s,NULL,NULL,'published',true,false,%s,%s,now(),now())""",
        (did, name, slug, description, json.dumps(layout), json.dumps(all_ids),
         json.dumps(filters or []), EMAIL, EMAIL))
    print(f"  dashboard: {name}  id={did}  ({len(layout)} blocks, {len(all_ids)} tiles)")
    return did


# ── Number-format viz presets ────────────────────────────────────────────────
KPI_BIG = {"chartTypeOptions": {"numberFormat": "auto"}}
KPI_PCT = {"chartTypeOptions": {"numberFormat": "fixed", "decimalPlaces": 2, "suffix": "%"}}
MAP_M = {"chartTypeOptions": {"mapNumberFormat": "m"}}


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
        ("population", "bigint", False, True),
        ("pct_affected", "double precision", False, True),
        ("cfr", "double precision", False, True),
    ])
    summary = dataset("COVID-19 Global Summary", "covid_global_summary", [
        ("total_population", "bigint", False, True),
        ("total_confirmed", "bigint", False, True),
        ("total_deaths", "bigint", False, True),
        ("cfr_pct", "double precision", False, True),
        ("pct_affected_pct", "double precision", False, True),
    ])
    print(f"datasets: daily={daily} country={country} summary={summary}")

    agg = "aggregate"
    ids = {}
    print("COVID charts:")

    # ── KPIs (4) ───────────────────────────────────────────────────────────────
    ids["kpi_pop"] = chart("Total Population", summary, "big_number",
          {"metrics": [M("total_population", "SUM", "People")], "query_mode": agg},
          desc="Combined population of all 196 countries in the dataset (JHU population lookup).",
          viz=KPI_BIG)
    ids["kpi_cases"] = chart("Total Confirmed Cases", country, "big_number",
          {"metrics": [M("confirmed", "SUM", "Cases")], "query_mode": agg},
          desc="Cumulative confirmed COVID-19 cases worldwide (JHU CSSE, full series).",
          viz=KPI_BIG)
    ids["kpi_deaths"] = chart("Total Deaths", country, "big_number",
          {"metrics": [M("deaths", "SUM", "Deaths")], "query_mode": agg},
          desc="Cumulative confirmed COVID-19 deaths worldwide.",
          viz=KPI_BIG)
    ids["kpi_cfr"] = chart("Case Fatality Rate", summary, "big_number",
          {"metrics": [M("cfr_pct", "SUM", "CFR")], "query_mode": agg},
          desc="Deaths ÷ confirmed cases, globally. A rough severity proxy — not the same as mortality rate.",
          viz=KPI_PCT)

    # ── Spread ──────────────────────────────────────────────────────────────────
    ids["map"] = chart("Confirmed Cases by Country", country, "world_map",
          {"metrics": [M("confirmed", "SUM", "Cases")], "groupby": ["country"],
           "sort_by": {"column": "confirmed", "direction": "desc"}, "query_mode": agg, "row_limit": 250},
          desc="Choropleth of cumulative cases. Click any country to cross-filter every chart below.",
          viz=MAP_M)

    # ── Trends ──────────────────────────────────────────────────────────────────
    ids["global_new"] = chart("Global New Cases (weekly)", daily, "time_series_line",
          {"metrics": [M("new_confirmed", "SUM", "New Cases")], "time_column": "dt", "time_grain": "week",
           "time_range": "all_time", "query_mode": agg, "row_limit": 5000},
          desc="New confirmed cases per week worldwide — the pandemic's wave structure.")
    ids["global_cum"] = chart("Global Cumulative Cases", daily, "time_series_area",
          {"metrics": [M("new_confirmed", "SUM", "Cumulative Cases")], "time_column": "dt", "time_grain": "week",
           "rolling_calc": "cumulative_sum", "time_range": "all_time", "query_mode": agg, "row_limit": 5000},
          desc="Running total of confirmed cases (weekly new cases accumulated).")
    ids["us_new"] = chart("US New Cases (weekly)", daily, "time_series_line",
          {"metrics": [M("new_confirmed", "SUM", "New Cases")], "time_column": "dt", "time_grain": "week",
           "filters": [{"column": "country", "op": "=", "value": "US"}],
           "time_range": "all_time", "query_mode": agg, "row_limit": 5000},
          desc="Weekly new cases in the United States (largest single-country caseload).")
    ids["share"] = chart("Deaths Share — Top 10", country, "donut",
          {"metrics": [M("deaths", "SUM", "Deaths")], "groupby": ["country"],
           "sort_by": {"column": "deaths", "direction": "desc"}, "query_mode": agg, "row_limit": 10},
          desc="Share of global deaths held by the 10 hardest-hit countries.")

    # ── Rankings ────────────────────────────────────────────────────────────────
    ids["top_cases"] = chart("Top 15 Countries by Cases", country, "bar_horizontal",
          {"metrics": [M("confirmed", "SUM", "Total Cases")], "groupby": ["country"],
           "sort_by": {"column": "confirmed", "direction": "desc"}, "query_mode": agg, "row_limit": 15},
          desc="Countries with the most cumulative confirmed cases.")
    ids["top_deaths"] = chart("Top 15 Countries by Deaths", country, "bar_horizontal",
          {"metrics": [M("deaths", "SUM", "Total Deaths")], "groupby": ["country"],
           "sort_by": {"column": "deaths", "direction": "desc"}, "query_mode": agg, "row_limit": 15},
          desc="Countries with the most cumulative confirmed deaths.")
    ids["pct_aff"] = chart("% Population Affected — Top 15", country, "bar_horizontal",
          {"metrics": [M("pct_affected", "SUM", "% Affected")], "groupby": ["country"],
           "sort_by": {"column": "pct_affected", "direction": "desc"}, "query_mode": agg, "row_limit": 15},
          desc="Confirmed cases as a share of national population — normalises for country size.",
          viz=KPI_PCT)
    ids["cfr_rank"] = chart("Case Fatality Rate — Top 15", country, "bar_horizontal",
          {"metrics": [M("cfr", "SUM", "CFR %")], "groupby": ["country"],
           "sort_by": {"column": "cfr", "direction": "desc"}, "query_mode": agg, "row_limit": 15},
          desc="Deaths ÷ cases by country. Highest ratios track fragile health systems and under-testing.",
          viz=KPI_PCT)

    # ── Detail table ─────────────────────────────────────────────────────────────
    ids["summary_tbl"] = chart("Country Detail", country, "table",
          {"metrics": [M("population", "SUM", "Population"), M("confirmed", "SUM", "Cases"),
                       M("deaths", "SUM", "Deaths"), M("pct_affected", "SUM", "% Affected"),
                       M("cfr", "SUM", "CFR %")],
           "groupby": ["country"],
           "sort_by": {"column": "confirmed", "direction": "desc"}, "query_mode": agg, "row_limit": 250},
          desc="Full per-country breakdown — searchable and sortable. Population, cases, deaths, and ratios.")

    print("chart ids:", {k: v for k, v in ids.items() if v})

    # ── Assemble the dashboard (positioned blocks, top → bottom) ──────────────────
    blocks, y = [], 0

    def add(block, h):
        nonlocal y
        blocks.append(block); y += h

    add(_text(
        "# COVID-19 Global Overview\n"
        "A live command-centre view of the pandemic, queried in real time from **Neon Postgres**. "
        "Every tile is interactive — **click a country on the map** (or any bar) to cross-filter the entire board. "
        "Source: *Johns Hopkins CSSE* time series + population lookup.", 3, y, size=14, color="#334155"), 3)

    add(_row([ids["kpi_pop"], ids["kpi_cases"], ids["kpi_deaths"], ids["kpi_cfr"]], 4, y), 4)

    add(_text("## 🗺  Global Spread\nCumulative confirmed cases by country. Darker = more cases. "
              "**Click a country** to focus every chart below on it.", 2, y, size=13, color="#475569"), 2)
    add(_row([ids["map"]], 9, y), 9)

    add(_text("## 📈  Trends Over Time\nHow the waves unfolded worldwide and in the United States.",
              2, y, size=13, color="#475569"), 2)
    add(_row([ids["global_new"], ids["global_cum"]], 7, y), 7)
    add(_row([ids["us_new"], ids["share"]], 7, y), 7)

    add(_text("## 🏆  Country Rankings\nWho was hit hardest — in absolute terms and relative to population.",
              2, y, size=13, color="#475569"), 2)
    add(_row([ids["top_cases"], ids["top_deaths"]], 8, y), 8)
    add(_row([ids["pct_aff"], ids["cfr_rank"]], 8, y), 8)

    add(_text("## 📋  Country Detail\nEvery country, every metric. Search and sort the full table.",
              2, y, size=13, color="#475569"), 2)
    add(_row([ids["summary_tbl"]], 10, y), 10)

    add(_text("---\n*Data: Johns Hopkins University CSSE (cases/deaths) + JHU population lookup. "
              "CFR is deaths ÷ confirmed cases and is sensitive to testing coverage. "
              "Demo dataset — figures reflect the loaded snapshot.*", 2, y, size=12, color="#94a3b8"), 2)

    missing = [k for k, v in ids.items() if not v]
    if missing:
        print(f"!! skipping dashboard — charts failed: {missing}")
        return

    dash("COVID-19 Global Overview",
         "Global COVID-19 cases, deaths, trends and country breakdowns — live from Neon Postgres.",
         blocks, filters=[_country_filter(country)])
    print("done.")
    cur.close(); conn.close()


if __name__ == "__main__":
    main()
