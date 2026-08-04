"""Load the 'COVID's impact on NYC' crossover dataset into Neon.

Joins, by date:
  • NYC COVID daily counts (DOHMH, rc75-m7u3): cases, hospitalizations, deaths
  • MTA daily ridership (data.ny.gov, vxuj-8kew): subway / bus / road (bridges &
    tunnels) volume + each as % of a comparable pre-pandemic day

→ covid_nyc_impact(dt, cases, deaths, hospitalized,
                   subway_riders, subway_pct, bus_pct, road_traffic, road_pct)

Shows the pandemic waves against the collapse & staggered recovery of NYC mobility.
Env: NEON_URL
"""
import os
import requests
import psycopg2
from psycopg2.extras import execute_values

URL = os.environ["NEON_URL"]
COVID = "https://data.cityofnewyork.us/resource/rc75-m7u3.json"
MTA = "https://data.ny.gov/resource/vxuj-8kew.json"


def get(url, params):
    r = requests.get(url, params=params, timeout=90); r.raise_for_status()
    return r.json()


def num(v):
    try:
        return float(v)
    except (TypeError, ValueError):
        return None


def main():
    print("Fetching NYC COVID daily + MTA ridership…")
    covid = get(COVID, {"$select": "date_of_interest,case_count,hospitalized_count,death_count",
                        "$order": "date_of_interest", "$limit": 50000})
    mta = get(MTA, {"$select": "date,subways_total_estimated_ridership,subways_of_comparable_pre_pandemic_day,"
                               "buses_of_comparable_pre_pandemic_day,bridges_and_tunnels_total_traffic,"
                               "bridges_and_tunnels_of_comparable_pre_pandemic_day",
                    "$order": "date", "$limit": 50000})

    rows = {}
    for r in covid:
        d = (r.get("date_of_interest") or "")[:10]
        if not d:
            continue
        rows[d] = {"cases": num(r.get("case_count")), "deaths": num(r.get("death_count")),
                   "hosp": num(r.get("hospitalized_count"))}
    for r in mta:
        d = (r.get("date") or "")[:10]
        if not d:
            continue
        e = rows.setdefault(d, {"cases": None, "deaths": None, "hosp": None})
        e["subway_riders"] = num(r.get("subways_total_estimated_ridership"))
        sp = num(r.get("subways_of_comparable_pre_pandemic_day"))
        bp = num(r.get("buses_of_comparable_pre_pandemic_day"))
        rp = num(r.get("bridges_and_tunnels_of_comparable_pre_pandemic_day"))
        e["subway_pct"] = round(sp * 100, 1) if sp is not None else None
        e["bus_pct"] = round(bp * 100, 1) if bp is not None else None
        e["road_pct"] = round(rp * 100, 1) if rp is not None else None
        e["road_traffic"] = num(r.get("bridges_and_tunnels_total_traffic"))

    out = []
    for d in sorted(rows):
        e = rows[d]
        out.append((d, e.get("cases"), e.get("deaths"), e.get("hosp"),
                    e.get("subway_riders"), e.get("subway_pct"), e.get("bus_pct"),
                    e.get("road_traffic"), e.get("road_pct")))

    conn = psycopg2.connect(URL, connect_timeout=60); conn.autocommit = False
    cur = conn.cursor()
    cur.execute("DROP TABLE IF EXISTS covid_nyc_impact")
    cur.execute("""
        CREATE TABLE covid_nyc_impact (
            dt DATE NOT NULL, cases DOUBLE PRECISION, deaths DOUBLE PRECISION,
            hospitalized DOUBLE PRECISION, subway_riders DOUBLE PRECISION,
            subway_pct DOUBLE PRECISION, bus_pct DOUBLE PRECISION,
            road_traffic DOUBLE PRECISION, road_pct DOUBLE PRECISION
        )
    """)
    execute_values(cur,
        "INSERT INTO covid_nyc_impact (dt,cases,deaths,hospitalized,subway_riders,subway_pct,bus_pct,road_traffic,road_pct) VALUES %s",
        out, page_size=2000)
    cur.execute("CREATE INDEX ix_nyc_impact_dt ON covid_nyc_impact(dt)")
    conn.commit()
    cur.execute("SELECT COUNT(*), MIN(dt), MAX(dt) FROM covid_nyc_impact")
    n, lo, hi = cur.fetchone()
    cur.execute("SELECT MIN(subway_pct), MAX(cases) FROM covid_nyc_impact")
    trough, peak = cur.fetchone()
    print(f"covid_nyc_impact: {n} days ({lo}..{hi}); subway trough {trough}% of pre-pandemic; peak daily cases {peak}")
    cur.close(); conn.close()


if __name__ == "__main__":
    main()
