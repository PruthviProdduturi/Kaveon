"""Load US state-level COVID-19 data (JHU CSSE US time series) into Neon.

Aggregates JHU county-level US series up to state level.

Creates:
  covid_us_daily(state, dt, confirmed, deaths, new_confirmed, new_deaths)
  covid_us_state_latest(state, confirmed, deaths, population, pct_affected, cfr)

Source: JHU CSSE time_series_covid19_{confirmed,deaths}_US.csv (public raw CSVs).
Env: NEON_URL
"""
import io
import os
import csv
from datetime import datetime

import requests
import psycopg2
from psycopg2.extras import execute_values

URL = os.environ["NEON_URL"]
BASE = ("https://raw.githubusercontent.com/CSSEGISandData/COVID-19/master/"
        "csse_covid_19_data/csse_covid_19_time_series/")
CONFIRMED = BASE + "time_series_covid19_confirmed_US.csv"
DEATHS = BASE + "time_series_covid19_deaths_US.csv"

# 50 states + DC — the entities the US map GeoJSON draws.
US_STATES = {
    "Alabama", "Alaska", "Arizona", "Arkansas", "California", "Colorado", "Connecticut",
    "Delaware", "District of Columbia", "Florida", "Georgia", "Hawaii", "Idaho", "Illinois",
    "Indiana", "Iowa", "Kansas", "Kentucky", "Louisiana", "Maine", "Maryland", "Massachusetts",
    "Michigan", "Minnesota", "Mississippi", "Missouri", "Montana", "Nebraska", "Nevada",
    "New Hampshire", "New Jersey", "New Mexico", "New York", "North Carolina", "North Dakota",
    "Ohio", "Oklahoma", "Oregon", "Pennsylvania", "Rhode Island", "South Carolina",
    "South Dakota", "Tennessee", "Texas", "Utah", "Vermont", "Virginia", "Washington",
    "West Virginia", "Wisconsin", "Wyoming",
}


def fetch(url):
    r = requests.get(url, timeout=120)
    r.raise_for_status()
    return list(csv.reader(io.StringIO(r.text)))


def main():
    print("Downloading JHU US confirmed + deaths…")
    conf = fetch(CONFIRMED)   # meta cols 0..10, dates from 11
    dth = fetch(DEATHS)       # meta cols 0..11 (Population at 11), dates from 12

    conf_dates = [datetime.strptime(d, "%m/%d/%y").date() for d in conf[0][11:]]
    n = len(conf_dates)

    state_conf = {}   # state -> [cumulative per date]
    state_dth = {}
    state_pop = {}

    for row in conf[1:]:
        st = (row[6] or "").strip()
        if st not in US_STATES:
            continue
        series = state_conf.setdefault(st, [0] * n)
        for i, v in enumerate(row[11:11 + n]):
            series[i] += int(v or 0)

    for row in dth[1:]:
        st = (row[6] or "").strip()
        if st not in US_STATES:
            continue
        try:
            state_pop[st] = state_pop.get(st, 0) + int(row[11] or 0)
        except ValueError:
            pass
        series = state_dth.setdefault(st, [0] * n)
        for i, v in enumerate(row[12:12 + n]):
            series[i] += int(v or 0)

    daily_rows, latest_rows = [], []
    for st in sorted(state_conf):
        c = state_conf[st]
        d = state_dth.get(st, [0] * n)
        prev_c = prev_d = 0
        for i, dt in enumerate(conf_dates):
            new_c = max(c[i] - prev_c, 0)
            new_d = max(d[i] - prev_d, 0)
            daily_rows.append((st, dt, c[i], d[i], new_c, new_d))
            prev_c, prev_d = c[i], d[i]
        pop = state_pop.get(st, 0)
        conf_last, dth_last = c[-1], d[-1]
        pct = round(conf_last / pop * 100, 3) if pop else None
        cfr = round(dth_last / conf_last * 100, 3) if conf_last else None
        latest_rows.append((st, conf_last, dth_last, pop or None, pct, cfr))

    conn = psycopg2.connect(URL, connect_timeout=60)
    conn.autocommit = False
    cur = conn.cursor()
    cur.execute("DROP TABLE IF EXISTS covid_us_daily")
    cur.execute("DROP TABLE IF EXISTS covid_us_state_latest")
    cur.execute("""
        CREATE TABLE covid_us_daily (
            state VARCHAR(60) NOT NULL, dt DATE NOT NULL,
            confirmed BIGINT NOT NULL, deaths BIGINT NOT NULL,
            new_confirmed BIGINT NOT NULL, new_deaths BIGINT NOT NULL
        )
    """)
    cur.execute("""
        CREATE TABLE covid_us_state_latest (
            state VARCHAR(60) NOT NULL, confirmed BIGINT NOT NULL, deaths BIGINT NOT NULL,
            population BIGINT, pct_affected DOUBLE PRECISION, cfr DOUBLE PRECISION
        )
    """)
    execute_values(cur,
        "INSERT INTO covid_us_daily (state,dt,confirmed,deaths,new_confirmed,new_deaths) VALUES %s",
        daily_rows, page_size=5000)
    execute_values(cur,
        "INSERT INTO covid_us_state_latest (state,confirmed,deaths,population,pct_affected,cfr) VALUES %s",
        latest_rows, page_size=200)
    cur.execute("CREATE INDEX ix_us_daily_dt ON covid_us_daily(dt)")
    cur.execute("CREATE INDEX ix_us_daily_state ON covid_us_daily(state)")
    conn.commit()
    cur.execute("SELECT COUNT(*) FROM covid_us_daily"); n1 = cur.fetchone()[0]
    cur.execute("SELECT COUNT(*) FROM covid_us_state_latest"); n2 = cur.fetchone()[0]
    cur.execute("SELECT state, confirmed FROM covid_us_state_latest ORDER BY confirmed DESC LIMIT 3")
    top = cur.fetchall()
    print(f"covid_us_daily: {n1} rows; covid_us_state_latest: {n2} states; top: {top}")
    cur.close(); conn.close()


if __name__ == "__main__":
    main()
