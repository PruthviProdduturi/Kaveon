"""Seed Kaveon's real telemetry tables (query_history + activity) with synthetic
events so the platform's built-in usage analytics look real and the DLM
usage_rollup (which counts query_history.tables_used) reflects hot tables.

Additive + idempotent-ish: removes prior synthetic rows (trigger_source='seed'
/ user domain @kaveon-demo.io) before inserting.

Run from apps/kaveon-api:  python ../../data/kaveon-usage/seed_telemetry.py
"""
import os
import sys
import time

_API = os.path.join(os.path.dirname(__file__), "..", "..", "apps", "kaveon-api")
sys.path.insert(0, os.path.abspath(_API))

import database.pool as pool  # noqa: E402

DB = "kaveon"

# real physical tables, weighted (repeats = hotter) for tables_used
TABLES = ("energy_annual,energy_annual,covid_global,covid_global,covid_global,"
          "temperature_monthly,leaderboard,leaderboard,benchmark_scores,arena_battles,"
          "pricing,kaveon_usage_daily,kaveon_usage_daily,climate_x_energy,nyc_taxi_borough")

QH_SQL = """
INSERT INTO query_history
  (id, sql_text, database_name, executed_at, execution_time, row_count, status,
   user_email, trigger_source, dataset_id, tables_used)
SELECT
  gen_random_uuid()::text,
  'SELECT * FROM ' || tbl || ' LIMIT 100',
  'kaveon',
  timestamp '2024-01-01' + (random()*525600)::int * interval '1 minute',
  round((0.02 + random()*2.4)::numeric, 3),
  (random()*8000)::int,
  CASE WHEN random() < 0.97 THEN 'success' ELSE 'error' END,
  'user' || (1 + floor(random()*3000)::int) || '@kaveon-demo.io',
  split_part('chat,chat,chat,sql_lab,sql_lab,dashboard,api', ',', 1 + floor(random()*7)::int),
  NULL,
  tbl
FROM (SELECT split_part('%s', ',', 1 + floor(random()*15)::int) AS tbl
      FROM generate_series(1, 120000) g) t
""" % TABLES

ACT_SQL = """
INSERT INTO activity (id, action, object_type, object_id, object_name, timestamp, user_email, details)
SELECT
  gen_random_uuid()::text,
  split_part('view,view,view,create,edit,edit,export,delete', ',', 1 + floor(random()*8)::int),
  ot,
  (1 + floor(random()*40)::int)::text,
  initcap(ot) || ' ' || (1 + floor(random()*40)::int),
  timestamp '2024-01-01' + (random()*525600)::int * interval '1 minute',
  'user' || (1 + floor(random()*3000)::int) || '@kaveon-demo.io',
  NULL
FROM (SELECT split_part('dataset,dashboard,dashboard,chart,chart,query', ',', 1 + floor(random()*6)::int) AS ot
      FROM generate_series(1, 40000) g) t
"""


def main():
    pool.execute_query("SELECT 1", DB)
    # clear prior synthetic seed rows
    pool.execute_query("DELETE FROM query_history WHERE user_email LIKE '%@kaveon-demo.io'", DB)
    pool.execute_query("DELETE FROM activity WHERE user_email LIKE '%@kaveon-demo.io'", DB)

    t0 = time.time()
    pool.execute_query(QH_SQL, DB)
    pool.execute_query(ACT_SQL, DB)
    sys.stderr.write("seeded telemetry in %.1fs\n" % (time.time() - t0))

    qh = pool.execute_query("SELECT COUNT(*) FROM query_history WHERE user_email LIKE '%@kaveon-demo.io'", DB)["rows"][0][0]
    ac = pool.execute_query("SELECT COUNT(*) FROM activity WHERE user_email LIKE '%@kaveon-demo.io'", DB)["rows"][0][0]
    hot = pool.execute_query(
        "SELECT tables_used, COUNT(*) c FROM query_history WHERE user_email LIKE '%@kaveon-demo.io' "
        "GROUP BY tables_used ORDER BY c DESC LIMIT 3", DB)["rows"]
    sys.stderr.write("query_history=%s activity=%s\n" % (qh, ac))
    sys.stderr.write("hottest tables: %s\n" % [(r[0], r[1]) for r in hot])


if __name__ == "__main__":
    main()
