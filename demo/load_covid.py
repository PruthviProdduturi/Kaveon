"""Load COVID-19 data (JHU CSSE) into the demo Postgres ($NEON_URL).

Creates two tables:
  covid_daily(country, dt, confirmed, deaths, new_confirmed, new_deaths)
  covid_country_latest(country, lat, lon, confirmed, deaths)

Source: https://github.com/CSSEGISandData/COVID-19 (public, stable raw CSVs).
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
CONFIRMED = BASE + "time_series_covid19_confirmed_global.csv"
DEATHS = BASE + "time_series_covid19_deaths_global.csv"


def fetch(url):
    r = requests.get(url, timeout=90)
    r.raise_for_status()
    return list(csv.reader(io.StringIO(r.text)))


def parse_series(rows):
    """Wide JHU CSV → {country: {date: cumulative}} and {country: (lat, lon)}."""
    header = rows[0]
    date_cols = header[4:]
    dates = [datetime.strptime(d, "%m/%d/%y").date() for d in date_cols]
    by_country = {}
    geo = {}
    for row in rows[1:]:
        country = row[1].strip()
        try:
            lat, lon = float(row[2] or 0), float(row[3] or 0)
        except ValueError:
            lat, lon = 0.0, 0.0
        if lat or lon:
            geo.setdefault(country, (lat, lon))  # first province as centroid
        series = by_country.setdefault(country, [0] * len(dates))
        for i, v in enumerate(row[4:]):
            series[i] += int(v or 0)
    return dates, by_country, geo


def main():
    print("Downloading JHU confirmed + deaths…")
    dates, confirmed, geo = parse_series(fetch(CONFIRMED))
    _, deaths, _ = parse_series(fetch(DEATHS))

    daily_rows = []
    latest_rows = []
    for country in sorted(confirmed):
        c = confirmed[country]
        d = deaths.get(country, [0] * len(dates))
        prev_c = prev_d = 0
        for i, dt in enumerate(dates):
            new_c = max(c[i] - prev_c, 0)
            new_d = max(d[i] - prev_d, 0)
            daily_rows.append((country, dt, c[i], d[i], new_c, new_d))
            prev_c, prev_d = c[i], d[i]
        lat, lon = geo.get(country, (0.0, 0.0))
        latest_rows.append((country, lat, lon, c[-1], d[-1]))

    conn = psycopg2.connect(URL, connect_timeout=60)
    conn.autocommit = False
    cur = conn.cursor()
    cur.execute("DROP TABLE IF EXISTS covid_daily")
    cur.execute("DROP TABLE IF EXISTS covid_country_latest")
    cur.execute("""
        CREATE TABLE covid_daily (
            country       VARCHAR(120) NOT NULL,
            dt            DATE         NOT NULL,
            confirmed     BIGINT       NOT NULL,
            deaths        BIGINT       NOT NULL,
            new_confirmed BIGINT       NOT NULL,
            new_deaths    BIGINT       NOT NULL
        )
    """)
    cur.execute("""
        CREATE TABLE covid_country_latest (
            country   VARCHAR(120) NOT NULL,
            lat       DOUBLE PRECISION,
            lon       DOUBLE PRECISION,
            confirmed BIGINT NOT NULL,
            deaths    BIGINT NOT NULL
        )
    """)
    execute_values(cur,
        "INSERT INTO covid_daily (country,dt,confirmed,deaths,new_confirmed,new_deaths) VALUES %s",
        daily_rows, page_size=5000)
    execute_values(cur,
        "INSERT INTO covid_country_latest (country,lat,lon,confirmed,deaths) VALUES %s",
        latest_rows, page_size=1000)
    cur.execute("CREATE INDEX ix_covid_daily_dt ON covid_daily(dt)")
    cur.execute("CREATE INDEX ix_covid_daily_country ON covid_daily(country)")
    conn.commit()
    cur.execute("SELECT COUNT(*) FROM covid_daily"); n1 = cur.fetchone()[0]
    cur.execute("SELECT COUNT(*) FROM covid_country_latest"); n2 = cur.fetchone()[0]
    cur.execute("SELECT MAX(dt) FROM covid_daily"); maxd = cur.fetchone()[0]
    print(f"covid_daily: {n1} rows (through {maxd}); covid_country_latest: {n2} countries")
    cur.close(); conn.close()


if __name__ == "__main__":
    main()
