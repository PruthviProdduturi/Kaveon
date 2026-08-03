"""Enrich covid_country_latest with population + derived ratios, and build a
one-row covid_global_summary table that powers the ratio KPIs (CFR, % affected).

Population source: JHU UID_ISO_FIPS_LookUp_Table.csv (same repo as the case data,
so Country_Region names align 1:1 with covid_country_latest.country).

Adds to covid_country_latest:  population, pct_affected (%), cfr (%)
Creates covid_global_summary:  total_population, total_confirmed, total_deaths,
                               cfr_pct, pct_affected_pct   (single row)

Env: NEON_URL
"""
import io
import os
import csv

import requests
import psycopg2

URL = os.environ["NEON_URL"]
LOOKUP = ("https://raw.githubusercontent.com/CSSEGISandData/COVID-19/master/"
          "csse_covid_19_data/UID_ISO_FIPS_LookUp_Table.csv")


def country_population():
    """Return {Country_Region: population}. Prefer the country-level row
    (Admin2 & Province_State empty); fall back to summing province rows."""
    r = requests.get(LOOKUP, timeout=90)
    r.raise_for_status()
    rows = list(csv.DictReader(io.StringIO(r.text)))
    country_total, province_sum = {}, {}
    for row in rows:
        cr = (row.get("Country_Region") or "").strip()
        if not cr:
            continue
        try:
            pop = int(float(row.get("Population") or 0))
        except ValueError:
            pop = 0
        admin2 = (row.get("Admin2") or "").strip()
        prov = (row.get("Province_State") or "").strip()
        if not admin2 and not prov:
            country_total[cr] = pop
        elif not admin2 and prov:
            province_sum[cr] = province_sum.get(cr, 0) + pop
    return {cr: (country_total.get(cr) or province_sum.get(cr) or 0)
            for cr in set(country_total) | set(province_sum)}


def main():
    pops = country_population()
    print(f"population lookup: {len(pops)} countries")

    conn = psycopg2.connect(URL, connect_timeout=60)
    conn.autocommit = True
    cur = conn.cursor()

    cur.execute("""
        ALTER TABLE covid_country_latest
          ADD COLUMN IF NOT EXISTS population   BIGINT,
          ADD COLUMN IF NOT EXISTS pct_affected DOUBLE PRECISION,
          ADD COLUMN IF NOT EXISTS cfr          DOUBLE PRECISION
    """)

    cur.execute("SELECT country FROM covid_country_latest")
    matched = 0
    for (country,) in cur.fetchall():
        pop = pops.get(country, 0)
        if pop:
            matched += 1
        cur.execute("UPDATE covid_country_latest SET population=%s WHERE country=%s",
                    (pop or None, country))
    print(f"matched population for {matched} of the loaded countries")

    # Drop non-country entities (cruise ships, Olympics, Antarctica) — they have no
    # population and pollute per-capita rankings and the map. Tiny case counts.
    cur.execute("SELECT country FROM covid_country_latest WHERE population IS NULL OR population = 0")
    non_countries = [r[0] for r in cur.fetchall()]
    if non_countries:
        cur.execute("DELETE FROM covid_country_latest WHERE country = ANY(%s)", (non_countries,))
        cur.execute("DELETE FROM covid_daily        WHERE country = ANY(%s)", (non_countries,))
        print(f"dropped {len(non_countries)} non-country entities: {', '.join(non_countries)}")

    cur.execute("""
        UPDATE covid_country_latest SET
          pct_affected = CASE WHEN population > 0
                              THEN ROUND((confirmed::numeric / population) * 100, 3) END,
          cfr          = CASE WHEN confirmed > 0
                              THEN ROUND((deaths::numeric / confirmed) * 100, 3) END
    """)

    cur.execute("DROP TABLE IF EXISTS covid_global_summary")
    cur.execute("""
        CREATE TABLE covid_global_summary AS
        SELECT
          SUM(population)::bigint                                        AS total_population,
          SUM(confirmed)::bigint                                        AS total_confirmed,
          SUM(deaths)::bigint                                           AS total_deaths,
          ROUND(SUM(deaths)::numeric   / NULLIF(SUM(confirmed),0) * 100, 2) AS cfr_pct,
          ROUND(SUM(confirmed)::numeric/ NULLIF(SUM(population),0) * 100, 2) AS pct_affected_pct
        FROM covid_country_latest
    """)

    cur.execute("SELECT * FROM covid_global_summary")
    cols = [d[0] for d in cur.description]
    print("global summary:", dict(zip(cols, cur.fetchone())))
    cur.close(); conn.close()


if __name__ == "__main__":
    main()
