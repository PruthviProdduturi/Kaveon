"""Seed a few example SQL Lab saved queries across the demo datasets.

Each query is verified to execute on Neon before it's saved. Idempotent: clears
prior seeds (by name) first. Plain Postgres SQL — runs as-is in SQL Lab against
the Neon Demo data source.

Env: NEON_URL
"""
import os
import psycopg2

URL = os.environ["NEON_URL"]
EMAIL = "pruthvi.prodduturi@gmail.com"

QUERIES = [
    ("🌍 Top 15 countries by cases",
     "Highest cumulative confirmed cases, with deaths.",
     "SELECT country, confirmed, deaths\n"
     "FROM covid_country_latest\n"
     "ORDER BY confirmed DESC\n"
     "LIMIT 15;"),
    ("🌍 Global new cases by week",
     "Weekly new confirmed cases worldwide — the pandemic waves.",
     "SELECT date_trunc('week', dt)::date AS week, SUM(new_confirmed) AS new_cases\n"
     "FROM covid_daily\n"
     "GROUP BY 1\nORDER BY 1;"),
    ("🇺🇸 US states by % population affected",
     "Cases as a share of state population (per-capita spread).",
     "SELECT state, confirmed, population,\n"
     "       ROUND(confirmed::numeric / NULLIF(population,0) * 100, 2) AS pct_affected\n"
     "FROM covid_us_state_latest\n"
     "ORDER BY pct_affected DESC\n"
     "LIMIT 15;"),
    ("🚕 NYC busiest pickup zones",
     "Top yellow-taxi pickup zones by trips (Jan 2023).",
     "SELECT zone, borough, trips, ROUND(revenue) AS revenue, avg_fare\n"
     "FROM nyc_taxi_zone\n"
     "ORDER BY trips DESC\n"
     "LIMIT 15;"),
    ("🚕 NYC taxi trips by hour of day",
     "When New Yorkers ride — pickups by hour (0–23).",
     "SELECT hour, trips, avg_fare\n"
     "FROM nyc_taxi_hourly\n"
     "ORDER BY hour;"),
    ("🗽 COVID vs subway ridership (monthly)",
     "The crossover: NYC cases against subway ridership as % of pre-pandemic.",
     "SELECT date_trunc('month', dt)::date AS month,\n"
     "       SUM(cases) AS cases,\n"
     "       ROUND(AVG(subway_pct)::numeric, 1) AS avg_subway_pct\n"
     "FROM covid_nyc_impact\n"
     "WHERE subway_pct IS NOT NULL\n"
     "GROUP BY 1\nORDER BY 1;"),
]


def main():
    conn = psycopg2.connect(URL, connect_timeout=30); conn.autocommit = True
    cur = conn.cursor()
    ok = 0
    for name, desc, sql in QUERIES:
        try:
            cur.execute(sql)
            rows = cur.fetchall()
        except Exception as e:
            print(f"  SKIP (query failed) {name}: {str(e)[:120]}")
            continue
        cur.execute("DELETE FROM saved_queries WHERE name = %s", (name,))
        cur.execute("""
            INSERT INTO saved_queries (name, description, sql_text, run_context, is_shared, tags,
                                       created_by, modified_by, created_at, modified_at)
            VALUES (%s,%s,%s,'sql-lab',true,'demo',%s,%s,now(),now())
        """, (name, desc, sql, EMAIL, EMAIL))
        print(f"  ok  {name}  ({len(rows)} rows)")
        ok += 1
    print(f"seeded {ok}/{len(QUERIES)} saved queries")
    cur.close(); conn.close()


if __name__ == "__main__":
    main()
