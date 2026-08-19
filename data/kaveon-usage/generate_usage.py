"""Generate synthetic Kaveon product-usage data (daily per-user rollup).

Server-side generation (generate_series + random) into the metadata DB:
  public.kaveon_users        — synthetic user dimension (plan/region/role/org/cohort)
  public.kaveon_usage_daily  — ~1M rows, one per user per day, weighted by plan +
                               weekday + a mild growth trend.

Run from apps/kaveon-api:  python ../../data/kaveon-usage/generate_usage.py
"""
import os
import sys
import time

# make the api package importable regardless of CWD
_API = os.path.join(os.path.dirname(__file__), "..", "..", "apps", "kaveon-api")
sys.path.insert(0, os.path.abspath(_API))

import database.pool as pool  # noqa: E402

DB = "kaveon"

# NOTE: the metadata pool's dialect layer rewrites [...] as bracket-identifiers,
# so ARRAY[...] literals break — use split_part over comma-delimited pools instead.
USERS_SQL = """
INSERT INTO public.kaveon_users
SELECT g,
  'user' || g || '@' || lower(replace(org, ' ', '')) || '.com',
  org, plan, region, role,
  (date '2023-01-01' + (random() * 640)::int)
FROM (
  SELECT g,
    split_part('Acme Corp,Globex,Initech,Umbrella,Stark Industries,Wayne Enterprises,Hooli,Pied Piper,Vandelay,Wonka Industries,Cyberdyne,Soylent,Massive Dynamic,Tyrell Corp,Aperture Labs', ',', 1 + floor(random()*15)::int) AS org,
    split_part('Free,Free,Free,Pro,Pro,Team,Team,Enterprise', ',', 1 + floor(random()*8)::int) AS plan,
    split_part('North America,North America,Europe,Europe,Asia,Asia,South America,Africa,Oceania', ',', 1 + floor(random()*9)::int) AS region,
    split_part('Viewer,Viewer,Viewer,Analyst,Analyst,Editor,Admin', ',', 1 + floor(random()*7)::int) AS role
  FROM generate_series(1, 3000) g
) t
"""

# 3000 users x 340 days = 1.02M rows. Metrics scale with plan weight (pw), drop on
# weekends (wf), and drift up slightly across the year (growth).
USAGE_SQL = """
INSERT INTO public.kaveon_usage_daily
SELECT
  row_number() OVER () AS id,
  ud AS usage_date,
  u.user_id,
  q AS queries_run,
  round(q * (0.30 + random()*0.40))::int AS nl_queries,
  round(q * (0.20 + random()*0.30))::int AS sql_lab_runs,
  greatest(0, round(pw * wf * (0.5 + random()*4))::int) AS dashboards_viewed,
  greatest(0, round(pw * wf * random()*1.5)::int) AS charts_created,
  greatest(0, round(pw * wf * (1 + random()*3))::int) AS datasets_accessed,
  greatest(0, round(pw * wf * random()*1.2)::int) AS exports,
  round((pw * wf * (5 + random()*40))::numeric, 1) AS active_minutes
FROM public.kaveon_users u
CROSS JOIN LATERAL generate_series(0, 339) AS s(day_off)
CROSS JOIN LATERAL (SELECT (date '2024-01-01' + s.day_off) AS ud) dd
CROSS JOIN LATERAL (
  SELECT
    (CASE u.plan WHEN 'Enterprise' THEN 8 WHEN 'Team' THEN 4 WHEN 'Pro' THEN 2 ELSE 1 END)::numeric
      * (0.7 + 0.6 * (s.day_off / 339.0)) AS pw,               -- +growth over the year
    (CASE WHEN extract(dow FROM (date '2024-01-01' + s.day_off)) IN (0, 6) THEN 0.35 ELSE 1.0 END)::numeric AS wf
) w
CROSS JOIN LATERAL (SELECT greatest(0, round(pw * wf * (0.5 + random()*3.5))::int) AS q) qq
"""


def main():
    pool.execute_query("SELECT 1", DB)  # warm up
    pool.execute_query("DROP TABLE IF EXISTS public.kaveon_usage_daily", DB)
    pool.execute_query("DROP TABLE IF EXISTS public.kaveon_users", DB)

    pool.execute_query("""
        CREATE TABLE public.kaveon_users (
          user_id INT PRIMARY KEY,
          user_email VARCHAR(80) NOT NULL,
          org VARCHAR(60) NOT NULL,
          plan VARCHAR(20) NOT NULL,
          region VARCHAR(30) NOT NULL,
          role VARCHAR(20) NOT NULL,
          signup_date DATE NOT NULL
        )""", DB)
    pool.execute_query(USERS_SQL, DB)
    n_users = pool.execute_query("SELECT COUNT(*) FROM public.kaveon_users", DB)["rows"][0][0]
    sys.stderr.write("users: %s\n" % n_users)

    pool.execute_query("""
        CREATE TABLE public.kaveon_usage_daily (
          id BIGINT PRIMARY KEY,
          usage_date DATE NOT NULL,
          user_id INT NOT NULL,
          queries_run INT NOT NULL,
          nl_queries INT NOT NULL,
          sql_lab_runs INT NOT NULL,
          dashboards_viewed INT NOT NULL,
          charts_created INT NOT NULL,
          datasets_accessed INT NOT NULL,
          exports INT NOT NULL,
          active_minutes NUMERIC(6,1) NOT NULL
        )""", DB)
    t0 = time.time()
    pool.execute_query(USAGE_SQL, DB)
    sys.stderr.write("usage_daily inserted in %.1fs\n" % (time.time() - t0))
    pool.execute_query("CREATE INDEX idx_usage_date ON public.kaveon_usage_daily (usage_date)", DB)
    pool.execute_query("CREATE INDEX idx_usage_user ON public.kaveon_usage_daily (user_id)", DB)

    stats = pool.execute_query(
        "SELECT COUNT(*), MIN(usage_date), MAX(usage_date), SUM(queries_run) "
        "FROM public.kaveon_usage_daily", DB)["rows"][0]
    sys.stderr.write("usage_daily: rows=%s range=%s..%s total_queries=%s\n" % tuple(stats))


if __name__ == "__main__":
    main()
