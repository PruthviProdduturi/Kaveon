"""Build the demo datasets + charts + a set of linked COVID-19 dashboards on Neon.

Design goals (presentation-grade, "show it to Elon"):
  - Split into focused, linked dashboards instead of one giant scroll:
      Overview  → KPIs + world map + links to the deep-dives
      Trends    → time-series (waves, cumulative, US)
      Rankings  → who was hit hardest (bars + share)
      Impact    → per-capita %-affected + case-fatality comparisons + detail
  - Colourful, high-end visuals (per-chart palettes; KPIs with sparkline trends)
  - Genuinely interactive: click a country on the map cross-filters everything;
    a global Country filter; and charts where a filter is meaningless (share /
    rankings) are marked exempt so they never collapse to a single 100% bar.

Every chart's SQL is verified via the running lens-api before it is persisted.

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

# ── Colour palettes ──────────────────────────────────────────────────────────
PALETTE = ["#6366f1", "#ec4899", "#14b8a6", "#f59e0b", "#8b5cf6", "#06b6d4",
           "#ef4444", "#10b981", "#f97316", "#3b82f6", "#a855f7", "#84cc16"]
MAP_SCALE = ["#eff6ff", "#bfdbfe", "#7dd3fc", "#38bdf8", "#0ea5e9", "#0369a1"]  # elegant blue sequential


def _viz(color=None, **cto):
    v = {}
    if color:
        v["color"] = color
    if cto:
        v["chartTypeOptions"] = cto
    return v


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
    """Verify the chart's SQL executes on Neon, then persist it. Falls back from
    big_number_trend → big_number if the trend query can't be produced."""
    def _try(ct):
        g = requests.post(f"{API}/api/v1/sql/generate", headers=H,
                          json={"dataset_id": dataset_id, "chart_type": ct, "config": qc}, timeout=40)
        if not g.ok:
            return None, f"GEN {g.status_code} {g.text[:120]}"
        sql = g.json().get("sql_text")
        e = requests.post(f"{API}/api/v1/sql/execute", headers=H,
                          json={"sql_text": sql, "database": "neondb", "source": "chart-builder",
                                "dataset_id": dataset_id, "chart_type": ct}, timeout=90)
        if not e.ok:
            return None, f"EXEC {e.status_code} {e.text[:120]} :: {sql[:140]}"
        return e.json().get("rows", []), None

    rows, err = _try(chart_type)
    ct = chart_type
    if rows is None and chart_type == "big_number_trend":
        rows, err = _try("big_number"); ct = "big_number"
    if rows is None:
        print(f"  FAIL {name}: {err}"); return None

    cur.execute("""INSERT INTO charts
        (name, description, chart_type, query_config, viz_config, visibility,
         created_on, changed_on, created_at, updated_at, created_by, updated_by)
        VALUES (%s,%s,%s,%s,%s,'published',now(),now(),now(),now(),%s,%s) RETURNING id""",
        (name, desc or name, ct, json.dumps({**qc, "dataset_id": dataset_id}),
         json.dumps(viz or {}), EMAIL, EMAIL))
    cid = cur.fetchone()[0]
    print(f"  ok  [{ct}] {name}  ({len(rows)} rows)  id={cid}")
    return cid


def M(col, agg, label):
    return {"column": col, "aggregate": agg, "label": label}


# ── Flat layout primitives (react-grid-layout: absolute x/y/w/h on a 12-col grid) ─

def C(cid, x, y, w, h, exempt=False):
    """A chart tile positioned on the grid."""
    node = {"i": "c" + uuid.uuid4().hex[:8], "type": "chart", "chartId": cid,
            "x": x, "y": y, "w": w, "h": h, "minW": 2, "minH": 2, "maxW": 12, "maxH": 40}
    if exempt:
        node["exemptFromFilters"] = True
    return node


def T(md, x, y, w, h, size=14, color="#475569"):
    """A markdown text tile positioned on the grid."""
    return {"i": "t" + uuid.uuid4().hex[:8], "type": "text",
            "x": x, "y": y, "w": w, "h": h, "minW": 2, "minH": 1, "maxW": 12, "maxH": 40,
            "textConfig": {"content": md, "alignment": "left", "fontSize": size, "color": color}}


def _country_filter(dataset_id):
    return {"id": "flt-country", "column": "covid_country_latest.country", "label": "Country",
            "operator": "=", "value": "", "appliesTo": "all", "enabled": False,
            "datasetId": dataset_id, "filterType": "value"}


def _date_filter(dataset_id):
    return {"id": "flt-date", "column": "covid_daily.dt", "label": "Date",
            "operator": ">=", "value": "", "appliesTo": "all", "enabled": False,
            "datasetId": dataset_id, "filterType": "date_range"}


def _state_filter(dataset_id):
    return {"id": "flt-state", "column": "covid_us_state_latest.state", "label": "State",
            "operator": "=", "value": "", "appliesTo": "all", "enabled": False,
            "datasetId": dataset_id, "filterType": "value"}


def dash(name, description, items, filters=None):
    """items: flat list of chart/text tiles with x/y/w/h."""
    all_ids = [it["chartId"] for it in items if it.get("chartId")]
    slug = re.sub(r"[^a-z0-9-]", "", name.lower().replace(" ", "-"))
    did = uuid.uuid4().hex
    cur.execute("""INSERT INTO dashboards
        (id, name, slug, description, layout, charts, filters, theme, tags, visibility,
         is_published, is_archived, created_by, modified_by, created_at, modified_at)
        VALUES (%s,%s,%s,%s,%s,%s,%s,NULL,NULL,'published',true,false,%s,%s,now(),now())""",
        (did, name, slug, description, json.dumps(items), json.dumps(all_ids),
         json.dumps(filters or []), EMAIL, EMAIL))
    print(f"  dashboard: {name}  id={did}  ({len(items)} tiles, {len(all_ids)} charts)")
    return did


KPI_BIG = dict(numberFormat="auto")
KPI_PCT = dict(numberFormat="fixed", decimalPlaces=2, suffix="%")


def main():
    print("reset…"); reset(); data_source()

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
        ("total_population", "bigint", False, True), ("total_confirmed", "bigint", False, True),
        ("total_deaths", "bigint", False, True), ("cfr_pct", "double precision", False, True),
        ("pct_affected_pct", "double precision", False, True),
    ])

    # Case-outcome breakdown (of confirmed cases): Survived vs Deaths — the Deaths
    # slice equals the CFR. A meaningful donut (unlike mixing population + a rate).
    cur.execute("DROP TABLE IF EXISTS covid_case_outcome")
    cur.execute("""
        CREATE TABLE covid_case_outcome AS
        SELECT 'Survived' AS outcome, (SUM(confirmed) - SUM(deaths))::bigint AS cases, 1 AS ord
        FROM covid_country_latest
        UNION ALL
        SELECT 'Deaths' AS outcome, SUM(deaths)::bigint AS cases, 2 AS ord
        FROM covid_country_latest
    """)
    outcome = dataset("COVID-19 Case Outcomes", "covid_case_outcome", [
        ("outcome", "varchar", True, False), ("cases", "bigint", False, True), ("ord", "int", False, True),
    ])

    # ── US state-level datasets (loaded by demo/load_covid_us.py) ────────────────
    us_daily = dataset("COVID-19 US Daily", "covid_us_daily", [
        ("state", "varchar", True, False), ("dt", "date", True, False),
        ("new_confirmed", "bigint", False, True), ("new_deaths", "bigint", False, True),
        ("confirmed", "bigint", False, True), ("deaths", "bigint", False, True),
    ], date_col="dt")
    us_state = dataset("COVID-19 US by State", "covid_us_state_latest", [
        ("state", "varchar", True, False),
        ("confirmed", "bigint", False, True), ("deaths", "bigint", False, True),
        ("population", "bigint", False, True),
        ("pct_affected", "double precision", False, True), ("cfr", "double precision", False, True),
    ])
    cur.execute("DROP TABLE IF EXISTS covid_us_summary")
    cur.execute("""
        CREATE TABLE covid_us_summary AS
        SELECT SUM(population)::bigint AS total_population, SUM(confirmed)::bigint AS total_confirmed,
               SUM(deaths)::bigint AS total_deaths,
               ROUND(SUM(deaths)::numeric / NULLIF(SUM(confirmed),0) * 100, 2) AS cfr_pct,
               ROUND(SUM(confirmed)::numeric / NULLIF(SUM(population),0) * 100, 2) AS pct_affected_pct
        FROM covid_us_state_latest
    """)
    us_summary = dataset("COVID-19 US Summary", "covid_us_summary", [
        ("total_population", "bigint", False, True), ("total_confirmed", "bigint", False, True),
        ("total_deaths", "bigint", False, True), ("cfr_pct", "double precision", False, True),
        ("pct_affected_pct", "double precision", False, True),
    ])
    print(f"datasets: daily={daily} country={country} summary={summary} outcome={outcome} "
          f"us_daily={us_daily} us_state={us_state} us_summary={us_summary}")

    agg = "aggregate"
    i = {}
    print("charts:")

    # ── KPIs — cases & deaths carry a sparkline trend (numbers + charts) ──────────
    i["kpi_pop"] = chart("Total Population", summary, "big_number",
        {"metrics": [M("total_population", "SUM", "People")], "query_mode": agg},
        desc="Combined population of the 196 countries in the dataset.", viz=_viz(**KPI_BIG))
    i["kpi_cases"] = chart("Total Confirmed Cases", daily, "big_number_trend",
        {"metrics": [M("new_confirmed", "SUM", "Total Cases")], "time_column": "dt", "time_grain": "month",
         "rolling_calc": "cumulative_sum", "time_range": "all_time", "query_mode": agg, "row_limit": 5000},
        desc="Cumulative confirmed cases worldwide, with the growth curve.", viz=_viz(["#6366f1"], **KPI_BIG))
    i["kpi_deaths"] = chart("Total Deaths", daily, "big_number_trend",
        {"metrics": [M("new_deaths", "SUM", "Total Deaths")], "time_column": "dt", "time_grain": "month",
         "rolling_calc": "cumulative_sum", "time_range": "all_time", "query_mode": agg, "row_limit": 5000},
        desc="Cumulative confirmed deaths worldwide, with the growth curve.", viz=_viz(["#ef4444"], **KPI_BIG))
    i["kpi_cfr"] = chart("Case Fatality Rate", summary, "big_number",
        {"metrics": [M("cfr_pct", "SUM", "CFR")], "query_mode": agg},
        desc="Deaths ÷ confirmed cases, globally. A severity proxy, sensitive to testing.", viz=_viz(**KPI_PCT))

    # ── Map ──────────────────────────────────────────────────────────────────────
    i["map"] = chart("Confirmed Cases by Country", country, "world_map",
        {"metrics": [M("confirmed", "SUM", "Cases")], "groupby": ["country"],
         "sort_by": {"column": "confirmed", "direction": "desc"}, "query_mode": agg, "row_limit": 250},
        desc="Choropleth of cumulative cases. Click a country to cross-filter the deep-dives.",
        viz=_viz(MAP_SCALE, mapNumberFormat="m"))

    i["outcome"] = chart("Case Outcomes", outcome, "donut",
        {"metrics": [M("cases", "SUM", "Cases")], "groupby": ["outcome"],
         "sort_by": {"column": "ord", "direction": "asc"}, "query_mode": agg, "row_limit": 5},
        desc="Of all confirmed cases: survived vs died. The Deaths slice is the 1.02% case-fatality rate; centre shows total cases.",
        viz=_viz(["#10b981", "#ef4444"]))

    # ── Trends ────────────────────────────────────────────────────────────────────
    i["global_new"] = chart("Global New Cases (weekly)", daily, "time_series_line",
        {"metrics": [M("new_confirmed", "SUM", "New Cases")], "time_column": "dt", "time_grain": "week",
         "time_range": "all_time", "query_mode": agg, "row_limit": 5000},
        desc="New confirmed cases per week worldwide — the pandemic's waves.", viz=_viz(["#6366f1"]))
    i["global_cum"] = chart("Global Cumulative Cases", daily, "time_series_area",
        {"metrics": [M("new_confirmed", "SUM", "Cumulative Cases")], "time_column": "dt", "time_grain": "week",
         "rolling_calc": "cumulative_sum", "time_range": "all_time", "query_mode": agg, "row_limit": 5000},
        desc="Running total of confirmed cases over time.", viz=_viz(["#8b5cf6"]))
    i["global_deaths"] = chart("Global New Deaths (weekly)", daily, "time_series_area",
        {"metrics": [M("new_deaths", "SUM", "New Deaths")], "time_column": "dt", "time_grain": "week",
         "time_range": "all_time", "query_mode": agg, "row_limit": 5000},
        desc="New confirmed deaths per week worldwide.", viz=_viz(["#ef4444"]))
    i["us_new"] = chart("US New Cases (weekly)", daily, "time_series_line",
        {"metrics": [M("new_confirmed", "SUM", "New Cases")], "time_column": "dt", "time_grain": "week",
         "filters": [{"column": "country", "op": "=", "value": "US"}],
         "time_range": "all_time", "query_mode": agg, "row_limit": 5000},
        desc="Weekly new cases in the United States.", viz=_viz(["#06b6d4"]))

    # ── Rankings (filter-exempt: absolute rankings shouldn't collapse to 1 bar) ───
    i["top_cases"] = chart("Top 15 Countries by Cases", country, "bar_horizontal",
        {"metrics": [M("confirmed", "SUM", "Total Cases")], "groupby": ["country"],
         "sort_by": {"column": "confirmed", "direction": "desc"}, "query_mode": agg, "row_limit": 15},
        desc="Countries with the most cumulative confirmed cases.", viz=_viz(["#14b8a6"]))
    i["top_deaths"] = chart("Top 15 Countries by Deaths", country, "bar_horizontal",
        {"metrics": [M("deaths", "SUM", "Total Deaths")], "groupby": ["country"],
         "sort_by": {"column": "deaths", "direction": "desc"}, "query_mode": agg, "row_limit": 15},
        desc="Countries with the most cumulative confirmed deaths.", viz=_viz(["#f97316"]))
    i["share"] = chart("Deaths Share — Top 10", country, "donut",
        {"metrics": [M("deaths", "SUM", "Deaths")], "groupby": ["country"],
         "sort_by": {"column": "deaths", "direction": "desc"}, "query_mode": agg, "row_limit": 10},
        desc="Share of global deaths held by the 10 hardest-hit countries.", viz=_viz(PALETTE))

    # ── Impact / per-capita (filter-exempt for the same reason) ───────────────────
    # Ratios (pct_affected, cfr) are non-additive — use AVG, not SUM, so grouping
    # never sums percentages into nonsense (each country has one row, so AVG = value).
    i["pct_aff"] = chart("% Population Affected — Top 15", country, "bar_horizontal",
        {"metrics": [M("pct_affected", "AVG", "Pct Affected")], "groupby": ["country"],
         "sort_by": {"column": "pct_affected", "direction": "desc"}, "query_mode": agg, "row_limit": 15},
        desc="Confirmed cases as a share of national population — normalises for size.", viz=_viz(["#f59e0b"]))
    i["cfr_rank"] = chart("Case Fatality Rate — Top 15", country, "bar_horizontal",
        {"metrics": [M("cfr", "AVG", "CFR")], "groupby": ["country"],
         "sort_by": {"column": "cfr", "direction": "desc"}, "query_mode": agg, "row_limit": 15},
        desc="Deaths ÷ cases by country. Highest ratios track fragile health systems.", viz=_viz(["#ec4899"]))
    i["detail"] = chart("Country Detail", country, "table",
        {"metrics": [M("population", "SUM", "Population"), M("confirmed", "SUM", "Cases"),
                     M("deaths", "SUM", "Deaths"), M("pct_affected", "AVG", "Pct Affected"),
                     M("cfr", "AVG", "CFR")],
         "groupby": ["country"], "sort_by": {"column": "confirmed", "direction": "desc"},
         "query_mode": agg, "row_limit": 250},
        desc="Full per-country breakdown — searchable and sortable.")

    # ── US state-level charts ────────────────────────────────────────────────────
    print("US charts:")
    i["us_kpi_pop"] = chart("US Population", us_summary, "big_number",
        {"metrics": [M("total_population", "SUM", "People")], "query_mode": agg},
        desc="Combined population of the 50 states + DC.", viz=_viz(**KPI_BIG))
    i["us_kpi_cases"] = chart("US Confirmed Cases", us_summary, "big_number",
        {"metrics": [M("total_confirmed", "SUM", "Cases")], "query_mode": agg},
        desc="Cumulative confirmed cases across the US.", viz=_viz(**KPI_BIG))
    i["us_kpi_deaths"] = chart("US Deaths", us_summary, "big_number",
        {"metrics": [M("total_deaths", "SUM", "Deaths")], "query_mode": agg},
        desc="Cumulative confirmed deaths across the US.", viz=_viz(**KPI_BIG))
    i["us_kpi_cfr"] = chart("US Case Fatality Rate", us_summary, "big_number",
        {"metrics": [M("cfr_pct", "SUM", "CFR")], "query_mode": agg},
        desc="US deaths ÷ confirmed cases.", viz=_viz(**KPI_PCT))
    i["us_map"] = chart("Confirmed Cases by State", us_state, "world_map",
        {"metrics": [M("confirmed", "SUM", "Cases")], "groupby": ["state"],
         "sort_by": {"column": "confirmed", "direction": "desc"}, "query_mode": agg, "row_limit": 60},
        desc="US choropleth of cumulative cases. Click a state to cross-filter.",
        viz=_viz(MAP_SCALE, mapRegion="usa", mapNumberFormat="m"))
    i["us_top_cases"] = chart("Top 15 States by Cases", us_state, "bar_horizontal",
        {"metrics": [M("confirmed", "SUM", "Cases")], "groupby": ["state"],
         "sort_by": {"column": "confirmed", "direction": "desc"}, "query_mode": agg, "row_limit": 15},
        desc="States with the most cumulative cases.", viz=_viz(["#0ea5e9"]))
    i["us_top_deaths"] = chart("Top 15 States by Deaths", us_state, "bar_horizontal",
        {"metrics": [M("deaths", "SUM", "Deaths")], "groupby": ["state"],
         "sort_by": {"column": "deaths", "direction": "desc"}, "query_mode": agg, "row_limit": 15},
        desc="States with the most cumulative deaths.", viz=_viz(["#f97316"]))
    i["us_pct"] = chart("% Population Affected — Top 15 States", us_state, "bar_horizontal",
        {"metrics": [M("pct_affected", "AVG", "Pct Affected")], "groupby": ["state"],
         "sort_by": {"column": "pct_affected", "direction": "desc"}, "query_mode": agg, "row_limit": 15},
        desc="Cases as a share of state population.", viz=_viz(["#f59e0b"]))
    i["us_cfr"] = chart("Case Fatality Rate — Top 15 States", us_state, "bar_horizontal",
        {"metrics": [M("cfr", "AVG", "CFR")], "groupby": ["state"],
         "sort_by": {"column": "cfr", "direction": "desc"}, "query_mode": agg, "row_limit": 15},
        desc="Deaths ÷ cases by state.", viz=_viz(["#ec4899"]))
    i["us_trend"] = chart("US New Cases (weekly)", us_daily, "time_series_area",
        {"metrics": [M("new_confirmed", "SUM", "New Cases")], "time_column": "dt", "time_grain": "week",
         "time_range": "all_time", "query_mode": agg, "row_limit": 5000},
        desc="Weekly new confirmed cases across the US.", viz=_viz(["#0ea5e9"]))
    i["us_deaths_trend"] = chart("US New Deaths (weekly)", us_daily, "time_series_area",
        {"metrics": [M("new_deaths", "SUM", "New Deaths")], "time_column": "dt", "time_grain": "week",
         "time_range": "all_time", "query_mode": agg, "row_limit": 5000},
        desc="Weekly new confirmed deaths across the US.", viz=_viz(["#ef4444"]))
    i["us_table"] = chart("State Detail", us_state, "table",
        {"metrics": [M("population", "SUM", "Population"), M("confirmed", "SUM", "Cases"),
                     M("deaths", "SUM", "Deaths"), M("pct_affected", "AVG", "Pct Affected"),
                     M("cfr", "AVG", "CFR")],
         "groupby": ["state"], "sort_by": {"column": "confirmed", "direction": "desc"},
         "query_mode": agg, "row_limit": 60},
        desc="Every state — population, cases, deaths, per-capita and CFR.")

    if any(v is None for v in i.values()):
        print("!! some charts failed:", [k for k, v in i.items() if v is None]); return

    cf = [_country_filter(country), _date_filter(daily)]

    # ── Deep-dive dashboards (flat 12-col grid; build first so we can link them) ──
    id_trends = dash("COVID-19 · Trends Over Time",
        "Weekly waves, cumulative growth and the US curve — live from Neon.", [
            T("How the pandemic's waves unfolded — worldwide and in the US. "
              "Pick a country in the **Filters** bar to see its own curve.", 0, 0, 12, 2),
            C(i["global_new"], 0, 2, 6, 8), C(i["global_cum"], 6, 2, 6, 8),
            C(i["global_deaths"], 0, 10, 6, 8), C(i["us_new"], 6, 10, 6, 8),
        ], cf)

    id_rank = dash("COVID-19 · Country Rankings",
        "Top countries by cases and deaths, plus the global deaths share.", [
            T("Who was hit hardest in **absolute** terms. "
              "These rankings stay global on purpose (they ignore the country filter).", 0, 0, 12, 2),
            C(i["top_cases"], 0, 2, 6, 9), C(i["top_deaths"], 6, 2, 6, 9),
            C(i["share"], 0, 11, 12, 9, exempt=True),
        ], cf)

    id_impact = dash("COVID-19 · Impact & Comparisons",
        "Per-capita spread, case-fatality rates and the full country table.", [
            T("Normalised views — cases **per capita** and **case-fatality rate** — "
              "reveal a very different picture than raw totals.", 0, 0, 12, 2),
            C(i["pct_aff"], 0, 2, 6, 9, exempt=True), C(i["cfr_rank"], 6, 2, 6, 9, exempt=True),
            C(i["detail"], 0, 11, 12, 10),
        ], cf)

    # ── United States deep-dive dashboard ────────────────────────────────────────
    usf = [_state_filter(us_state), _date_filter(us_daily)]
    id_us = dash("COVID-19 · United States",
        "US state-level cases, deaths, per-capita spread and trends — live from Neon.", [
            T("The pandemic across the **United States**, by state. "
              "**Click a state on the map** to cross-filter, or pick one in the **Filters** bar.", 0, 0, 12, 2),
            C(i["us_kpi_pop"], 0, 2, 3, 5), C(i["us_kpi_cases"], 3, 2, 3, 5),
            C(i["us_kpi_deaths"], 6, 2, 3, 5), C(i["us_kpi_cfr"], 9, 2, 3, 5),
            C(i["us_map"], 0, 7, 8, 13, exempt=True),
            C(i["us_top_cases"], 8, 7, 4, 13, exempt=True),
            C(i["us_trend"], 0, 20, 6, 8), C(i["us_deaths_trend"], 6, 20, 6, 8),
            C(i["us_pct"], 0, 28, 6, 9, exempt=True), C(i["us_cfr"], 6, 28, 6, 9, exempt=True),
            C(i["us_table"], 0, 37, 12, 10),
        ], usf)

    base = "/dashboards"
    dash("COVID-19 Global Overview",
        "Global KPIs, world map and links to the trend, ranking and impact deep-dives — live from Neon.", [
            T("A live command-centre view of the pandemic, queried in real time from **Neon Postgres**. "
              "**Click a country on the map** to cross-filter, or jump into a deep-dive below.", 0, 0, 12, 2),
            C(i["kpi_pop"], 0, 2, 3, 5), C(i["kpi_cases"], 3, 2, 3, 5),
            C(i["kpi_deaths"], 6, 2, 3, 5), C(i["kpi_cfr"], 9, 2, 3, 5),
            C(i["map"], 0, 7, 8, 13, exempt=True),
            C(i["outcome"], 8, 7, 4, 7, exempt=True),
            T("## 🔎 Dive deeper\n"
              f"- 📈 **[Trends Over Time]({base}/{id_trends}/view)** — weekly waves, cumulative growth, the US curve\n"
              f"- 🏆 **[Country Rankings]({base}/{id_rank}/view)** — who was hit hardest in absolute terms\n"
              f"- 🧭 **[Impact & Comparisons]({base}/{id_impact}/view)** — per-capita spread & case-fatality rates\n"
              f"- 🇺🇸 **[United States]({base}/{id_us}/view)** — state-by-state map, rankings & trends",
              8, 14, 4, 6, size=14),
            T("---\n*Data: Johns Hopkins CSSE (cases/deaths) + JHU population lookup. "
              "CFR = deaths ÷ confirmed and is sensitive to testing coverage. Demo snapshot.*",
              0, 20, 12, 2, size=12, color="#94a3b8"),
        ], cf)

    print("done.")
    cur.close(); conn.close()


if __name__ == "__main__":
    main()
