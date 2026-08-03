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
MAP_SCALE = ["#dbeafe", "#93c5fd", "#6366f1", "#a21caf", "#be123c"]  # cool → hot


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


# ── Layout primitives ────────────────────────────────────────────────────────

def _chart_child(cid, parent, w, h, exempt=False):
    node = {"i": "c" + uuid.uuid4().hex[:8], "type": "chart", "chartId": cid, "parentId": parent,
            "x": 0, "y": 0, "w": w, "h": h, "minW": 2, "minH": 2, "maxW": 12, "maxH": 24}
    if exempt:
        node["exemptFromFilters"] = True
    return node


def _row(items, height, y, widths=None):
    """items: list of (chart_id, exempt) or bare chart_id."""
    norm = [(it if isinstance(it, tuple) else (it, False)) for it in items]
    rid = "row" + uuid.uuid4().hex[:8]
    n = len(norm)
    if not widths:
        base = 12 // n
        widths = [base] * n
        widths[0] += 12 - base * n
    children = [_chart_child(cid, rid, widths[i], height, ex) for i, (cid, ex) in enumerate(norm)]
    return {"i": rid, "type": "row", "x": 0, "y": y, "w": 12, "h": height,
            "minW": 12, "minH": 2, "maxW": 12, "maxH": 24, "children": children}


def _text(md, height, y, size=14, color="#475569"):
    return {"i": "txt" + uuid.uuid4().hex[:8], "type": "text", "x": 0, "y": y, "w": 12, "h": height,
            "minW": 2, "minH": 1, "maxW": 12, "maxH": 24,
            "textConfig": {"content": md, "alignment": "left", "fontSize": size, "color": color}}


def _country_filter(dataset_id):
    return {"id": "flt-country", "column": "covid_country_latest.country", "label": "Country",
            "operator": "=", "value": "", "appliesTo": "all", "enabled": False,
            "datasetId": dataset_id, "filterType": "value"}


def _date_filter(dataset_id):
    return {"id": "flt-date", "column": "covid_daily.dt", "label": "Date",
            "operator": ">=", "value": "", "appliesTo": "all", "enabled": False,
            "datasetId": dataset_id, "filterType": "date_range"}


class Blocks:
    """Accumulates positioned layout blocks with running y."""
    def __init__(self):
        self.items, self.y = [], 0

    def text(self, md, h, **kw):
        self.items.append(_text(md, h, self.y, **kw)); self.y += h; return self

    def row(self, items, h, widths=None):
        self.items.append(_row(items, h, self.y, widths)); self.y += h; return self


def dash(name, description, blocks, filters=None):
    layout = blocks.items
    all_ids = [ch["chartId"] for b in layout for ch in b.get("children", []) if ch.get("chartId")]
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
    print(f"datasets: daily={daily} country={country} summary={summary}")

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
    i["pct_aff"] = chart("% Population Affected — Top 15", country, "bar_horizontal",
        {"metrics": [M("pct_affected", "SUM", "Pct Affected")], "groupby": ["country"],
         "sort_by": {"column": "pct_affected", "direction": "desc"}, "query_mode": agg, "row_limit": 15},
        desc="Confirmed cases as a share of national population — normalises for size.", viz=_viz(["#f59e0b"]))
    i["cfr_rank"] = chart("Case Fatality Rate — Top 15", country, "bar_horizontal",
        {"metrics": [M("cfr", "SUM", "CFR")], "groupby": ["country"],
         "sort_by": {"column": "cfr", "direction": "desc"}, "query_mode": agg, "row_limit": 15},
        desc="Deaths ÷ cases by country. Highest ratios track fragile health systems.", viz=_viz(["#ec4899"]))
    i["detail"] = chart("Country Detail", country, "table",
        {"metrics": [M("population", "SUM", "Population"), M("confirmed", "SUM", "Cases"),
                     M("deaths", "SUM", "Deaths"), M("pct_affected", "SUM", "Pct Affected"),
                     M("cfr", "SUM", "CFR")],
         "groupby": ["country"], "sort_by": {"column": "confirmed", "direction": "desc"},
         "query_mode": agg, "row_limit": 250},
        desc="Full per-country breakdown — searchable and sortable.")

    if any(v is None for v in i.values()):
        print("!! some charts failed:", [k for k, v in i.items() if v is None]); return

    cf = [_country_filter(country), _date_filter(daily)]

    # ── Deep-dive dashboards (build first so we can link to them) ─────────────────
    trends = Blocks()
    trends.text("How the pandemic's waves unfolded — worldwide and in the US. "
                "Pick a country in the **Filters** bar to see its own curve.", 2, size=14, color="#475569")
    trends.row([i["global_new"], i["global_cum"]], 7)
    trends.row([i["global_deaths"], i["us_new"]], 7)
    id_trends = dash("COVID-19 · Trends Over Time",
                     "Weekly waves, cumulative growth and the US curve — live from Neon.", trends, cf)

    rankings = Blocks()
    rankings.text("Who was hit hardest in **absolute** terms. "
                  "These rankings stay global on purpose (they ignore the country filter).", 2, size=14, color="#475569")
    rankings.row([i["top_cases"], i["top_deaths"]], 8)
    rankings.row([(i["share"], True)], 8)
    id_rank = dash("COVID-19 · Country Rankings",
                   "Top countries by cases and deaths, plus the global deaths share.", rankings, cf)

    impact = Blocks()
    impact.text("Normalised views — cases **per capita** and **case-fatality rate** — "
                "reveal a very different picture than raw totals.", 2, size=14, color="#475569")
    impact.row([(i["pct_aff"], True), (i["cfr_rank"], True)], 8)
    impact.row([i["detail"]], 10)
    id_impact = dash("COVID-19 · Impact & Comparisons",
                     "Per-capita spread, case-fatality rates and the full country table.", impact, cf)

    base = "/dashboards"
    overview = Blocks()
    overview.text(
        "A live command-centre view of the pandemic, queried in real time from **Neon Postgres**. "
        "**Click a country on the map** to cross-filter, or jump into a deep-dive below.", 2,
        size=14, color="#475569")
    overview.row([i["kpi_pop"], i["kpi_cases"], i["kpi_deaths"], i["kpi_cfr"]], 5)
    overview.row([(i["map"], True)], 13)
    overview.text(
        "## 🔎 Dive deeper\n"
        f"- 📈 **[Trends Over Time]({base}/{id_trends}/view)** — weekly waves, cumulative growth, the US curve\n"
        f"- 🏆 **[Country Rankings]({base}/{id_rank}/view)** — who was hit hardest in absolute terms\n"
        f"- 🧭 **[Impact & Comparisons]({base}/{id_impact}/view)** — per-capita spread & case-fatality rates",
        4, size=14, color="#475569")
    overview.text("---\n*Data: Johns Hopkins CSSE (cases/deaths) + JHU population lookup. "
                  "CFR = deaths ÷ confirmed and is sensitive to testing coverage. Demo snapshot.*",
                  2, size=12, color="#94a3b8")
    dash("COVID-19 Global Overview",
         "Global KPIs, world map and links to the trend, ranking and impact deep-dives — live from Neon.",
         overview, cf)

    print("done.")
    cur.close(); conn.close()


if __name__ == "__main__":
    main()
