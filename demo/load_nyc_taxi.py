"""Load NYC Yellow Taxi trip data (one month) into Neon, aggregated for dashboards.

Downloads the TLC yellow-taxi parquet for one month, aggregates with pyarrow
(no full row load into Python), joins pickup zones to boroughs, and writes:

  nyc_taxi_summary(total_trips, total_revenue, avg_fare, avg_distance, avg_tip)   -- 1 row
  nyc_taxi_borough(borough, trips, revenue, avg_fare, avg_distance)               -- 5-6 rows
  nyc_taxi_zone(zone, borough, trips, revenue, avg_fare, avg_distance)            -- ~260 rows
  nyc_taxi_hourly(hour, trips, avg_fare)                                          -- 24 rows
  nyc_taxi_daily(dt, trips, revenue)                                             -- ~31 rows

Env: NEON_URL
"""
import io
import os
import csv
import tempfile
from datetime import date

import requests
import psycopg2
from psycopg2.extras import execute_values
import pyarrow as pa
import pyarrow.parquet as pq
import pyarrow.compute as pc

URL = os.environ["NEON_URL"]
MONTH = "2023-01"
PARQUET = f"https://d37ci6vzurychx.cloudfront.net/trip-data/yellow_tripdata_{MONTH}.parquet"
LOOKUP = "https://d37ci6vzurychx.cloudfront.net/misc/taxi_zone_lookup.csv"
Y, M = int(MONTH[:4]), int(MONTH[5:7])


def zone_lookup():
    r = requests.get(LOOKUP, timeout=60); r.raise_for_status()
    out = {}
    for row in csv.DictReader(io.StringIO(r.text)):
        out[int(row["LocationID"])] = (row["Borough"], row["Zone"])
    return out


def main():
    print(f"Downloading TLC yellow taxi {MONTH} parquet…")
    r = requests.get(PARQUET, timeout=300); r.raise_for_status()
    tmp = os.path.join(tempfile.gettempdir(), f"yellow_{MONTH}.parquet")
    open(tmp, "wb").write(r.content)
    print(f"  {len(r.content)//1_000_000} MB")

    cols = ["tpep_pickup_datetime", "PULocationID", "trip_distance", "fare_amount",
            "total_amount", "tip_amount"]
    t = pq.read_table(tmp, columns=cols)
    ts = t["tpep_pickup_datetime"]
    d = pc.cast(ts, pa.date32())
    t = t.append_column("hour", pc.hour(ts)).append_column("pdate", d)

    # Sanity filters: within-month, sane fares/distances.
    lo, hi = date(Y, M, 1), (date(Y, M + 1, 1) if M < 12 else date(Y + 1, 1, 1))
    mask = pc.and_(pc.greater_equal(t["pdate"], pa.scalar(lo)), pc.less(t["pdate"], pa.scalar(hi)))
    mask = pc.and_(mask, pc.greater(t["total_amount"], 0))
    mask = pc.and_(mask, pc.less(t["total_amount"], 1000))
    mask = pc.and_(mask, pc.greater_equal(t["trip_distance"], 0))
    mask = pc.and_(mask, pc.less(t["trip_distance"], 200))
    t = t.filter(mask)
    print(f"  {t.num_rows:,} trips after filtering")

    lk = zone_lookup()

    def agg(keys):
        g = t.group_by(keys).aggregate([
            ("PULocationID", "count"), ("total_amount", "sum"),
            ("fare_amount", "mean"), ("trip_distance", "mean"), ("tip_amount", "mean"),
        ])
        return g.to_pylist()

    # ── by zone ──
    zrows = []
    for r_ in agg(["PULocationID"]):
        loc = r_["PULocationID"]
        borough, zone = lk.get(loc, ("Unknown", f"Zone {loc}"))
        zrows.append((zone, borough, r_["PULocationID_count"], round(r_["total_amount_sum"], 2),
                      round(r_["fare_amount_mean"], 2), round(r_["trip_distance_mean"], 3)))

    # ── by borough ──
    bagg = {}
    for r_ in agg(["PULocationID"]):
        loc = r_["PULocationID"]
        borough = lk.get(loc, ("Unknown", ""))[0]
        b = bagg.setdefault(borough, {"trips": 0, "rev": 0.0, "fare_w": 0.0, "dist_w": 0.0})
        cnt = r_["PULocationID_count"]
        b["trips"] += cnt; b["rev"] += r_["total_amount_sum"]
        b["fare_w"] += r_["fare_amount_mean"] * cnt; b["dist_w"] += r_["trip_distance_mean"] * cnt
    brows = [(bk, v["trips"], round(v["rev"], 2), round(v["fare_w"] / v["trips"], 2),
              round(v["dist_w"] / v["trips"], 3)) for bk, v in bagg.items()]

    # ── by hour ──
    hrows = [(r_["hour"], r_["PULocationID_count"], round(r_["fare_amount_mean"], 2))
             for r_ in sorted(agg(["hour"]), key=lambda x: x["hour"])]

    # ── by day ──
    drows = [(r_["pdate"], r_["PULocationID_count"], round(r_["total_amount_sum"], 2))
             for r_ in sorted(agg(["pdate"]), key=lambda x: x["pdate"])]

    total_trips = t.num_rows
    total_rev = round(pc.sum(t["total_amount"]).as_py(), 2)
    avg_fare = round(pc.mean(t["fare_amount"]).as_py(), 2)
    avg_dist = round(pc.mean(t["trip_distance"]).as_py(), 3)
    avg_tip = round(pc.mean(t["tip_amount"]).as_py(), 2)

    conn = psycopg2.connect(URL, connect_timeout=60); conn.autocommit = False
    cur = conn.cursor()
    for tbl in ["nyc_taxi_summary", "nyc_taxi_borough", "nyc_taxi_zone", "nyc_taxi_hourly", "nyc_taxi_daily"]:
        cur.execute(f"DROP TABLE IF EXISTS {tbl}")
    cur.execute("CREATE TABLE nyc_taxi_summary (total_trips BIGINT, total_revenue DOUBLE PRECISION, avg_fare DOUBLE PRECISION, avg_distance DOUBLE PRECISION, avg_tip DOUBLE PRECISION)")
    cur.execute("CREATE TABLE nyc_taxi_borough (borough VARCHAR(40), trips BIGINT, revenue DOUBLE PRECISION, avg_fare DOUBLE PRECISION, avg_distance DOUBLE PRECISION)")
    cur.execute("CREATE TABLE nyc_taxi_zone (zone VARCHAR(80), borough VARCHAR(40), trips BIGINT, revenue DOUBLE PRECISION, avg_fare DOUBLE PRECISION, avg_distance DOUBLE PRECISION)")
    cur.execute("CREATE TABLE nyc_taxi_hourly (hour INT, trips BIGINT, avg_fare DOUBLE PRECISION)")
    cur.execute("CREATE TABLE nyc_taxi_daily (dt DATE, trips BIGINT, revenue DOUBLE PRECISION)")
    cur.execute("INSERT INTO nyc_taxi_summary VALUES (%s,%s,%s,%s,%s)", (total_trips, total_rev, avg_fare, avg_dist, avg_tip))
    execute_values(cur, "INSERT INTO nyc_taxi_borough (borough,trips,revenue,avg_fare,avg_distance) VALUES %s", brows)
    execute_values(cur, "INSERT INTO nyc_taxi_zone (zone,borough,trips,revenue,avg_fare,avg_distance) VALUES %s", zrows, page_size=500)
    execute_values(cur, "INSERT INTO nyc_taxi_hourly (hour,trips,avg_fare) VALUES %s", hrows)
    execute_values(cur, "INSERT INTO nyc_taxi_daily (dt,trips,revenue) VALUES %s", drows)
    conn.commit()
    print(f"summary: {total_trips:,} trips, ${total_rev:,.0f} revenue, ${avg_fare} avg fare, {avg_dist} mi avg")
    print(f"boroughs: {len(brows)}, zones: {len(zrows)}, hours: {len(hrows)}, days: {len(drows)}")
    cur.close(); conn.close()


if __name__ == "__main__":
    main()
