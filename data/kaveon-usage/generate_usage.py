"""Generate synthetic Kaveon product-usage data (daily per-user rollup).

Server-side generation (generate_series + random) into the metadata DB:
  public.kaveon_users        — synthetic user dimension table
  public.kaveon_usage_daily  — ~10M rows, one per user per day, weighted by plan +
                               weekday + a mild growth trend.

Dimensions: org, plan, region, country, role, segment, sub_segment, industry,
            acquisition_channel, signup_date.

Run from apps/kaveon-api:  python ../../data/kaveon-usage/generate_usage.py
"""
import os
import sys
import time

_API = os.path.join(os.path.dirname(__file__), "..", "..", "apps", "kaveon-api")
sys.path.insert(0, os.path.abspath(_API))

import database.pool as pool  # noqa: E402

DB = "kaveon"

_RC_POOL = (
    "North America~United States,North America~United States,North America~United States,"
    "North America~Canada,North America~Mexico,"
    "Europe~United Kingdom,Europe~Germany,Europe~France,Europe~Netherlands,"
    "Europe~Spain,Europe~Sweden,Europe~Italy,"
    "Asia~India,Asia~India,Asia~Japan,Asia~Singapore,"
    "Asia~South Korea,Asia~Indonesia,Asia~China,"
    "South America~Brazil,South America~Argentina,South America~Colombia,South America~Chile,"
    "Africa~Nigeria,Africa~South Africa,Africa~Kenya,Africa~Egypt,"
    "Oceania~Australia,Oceania~New Zealand"
)
_RC_N = _RC_POOL.count(",") + 1

_SEG_POOL = (
    "Enterprise~Fortune 500,Enterprise~Fortune 500,Enterprise~Global 2000,Enterprise~Large Enterprise,"
    "Mid-Market~Growth Stage,Mid-Market~Established,Mid-Market~Regional Leader,"
    "SMB~Small Business,SMB~Small Business,SMB~Micro Business,"
    "Startup~Series A+,Startup~Pre-Seed,Startup~Bootstrapped,Individual~Solo"
)
_SEG_N = _SEG_POOL.count(",") + 1

USERS_SQL = f"""
INSERT INTO public.kaveon_users
SELECT g,
  'user' || g || '@' || lower(replace(org, ' ', '')) || '.com',
  org, plan,
  split_part(rc_pair, '~', 1) AS region,
  split_part(rc_pair, '~', 2) AS country,
  role,
  split_part(seg_pair, '~', 1) AS segment,
  split_part(seg_pair, '~', 2) AS sub_segment,
  industry, acq,
  (date '2024-01-01' + (random() * 940)::int)
FROM (
  SELECT g,
    split_part('Acme Corp,Globex,Initech,Umbrella,Stark Industries,Wayne Enterprises,Hooli,Pied Piper,Vandelay,Wonka Industries,Cyberdyne,Soylent,Massive Dynamic,Tyrell Corp,Aperture Labs', ',', 1 + floor(random()*15)::int) AS org,
    split_part('Free,Free,Free,Pro,Pro,Team,Team,Enterprise', ',', 1 + floor(random()*8)::int) AS plan,
    split_part('{_RC_POOL}', ',', 1 + floor(random()*{_RC_N} + g*0)::int) AS rc_pair,
    split_part('Viewer,Viewer,Viewer,Analyst,Analyst,Editor,Admin', ',', 1 + floor(random()*7)::int) AS role,
    split_part('{_SEG_POOL}', ',', 1 + floor(random()*{_SEG_N} + g*0)::int) AS seg_pair,
    split_part('Technology,Technology,Healthcare,Financial Services,Manufacturing,Retail,Education,Media,Energy,Government,Logistics,Real Estate', ',', 1 + floor(random()*12)::int) AS industry,
    split_part('Organic,Organic,Organic,Referral,Referral,Partner,Paid,Paid,Direct', ',', 1 + floor(random()*9)::int) AS acq
  FROM generate_series(1, 44000) g
) t
"""

USAGE_SQL = """
INSERT INTO public.kaveon_usage_daily
SELECT
  row_number() OVER () AS id,
  ud AS usage_date,
  u.user_id,
  q AS queries_run,
  round(q * (0.30 + random()*0.40))::int AS nl_queries,
  round(q * (0.20 + random()*0.30))::int AS sql_lab_runs,
  least(q, greatest(0, round(pw * wf * (0.3 + random()*2.0))::int)) AS dashboards_viewed,
  greatest(0, round(pw * wf * random()*1.5)::int) AS charts_created,
  greatest(0, round(pw * wf * (1 + random()*3))::int) AS datasets_accessed,
  greatest(0, round(pw * wf * random()*1.2)::int) AS exports,
  round((pw * wf * (5 + random()*40))::numeric, 1) AS active_minutes,
  u.org, u.plan, u.region, u.country, u.role,
  u.segment, u.sub_segment, u.industry, u.acquisition_channel,
  u.signup_date
FROM public.kaveon_users u
CROSS JOIN LATERAL generate_series(0, 229) AS s(day_off)
CROSS JOIN LATERAL (SELECT (date '2026-01-01' + s.day_off) AS ud) dd
CROSS JOIN LATERAL (
  SELECT
    (CASE u.plan WHEN 'Enterprise' THEN 8 WHEN 'Team' THEN 4 WHEN 'Pro' THEN 2 ELSE 1 END)::numeric
      * (0.7 + 0.6 * (s.day_off / 229.0)) AS pw,
    (CASE WHEN extract(dow FROM (date '2026-01-01' + s.day_off)) IN (0, 6) THEN 0.35 ELSE 1.0 END)::numeric AS wf
) w
CROSS JOIN LATERAL (SELECT greatest(0, round(pw * wf * (0.5 + random()*3.5))::int) AS q) qq
"""


def main():
    pool.execute_query("SELECT 1", DB)
    pool.execute_query("DROP TABLE IF EXISTS public.kaveon_usage_daily", DB)
    pool.execute_query("DROP TABLE IF EXISTS public.kaveon_users", DB)

    pool.execute_query("""
        CREATE TABLE public.kaveon_users (
          user_id INT PRIMARY KEY,
          user_email VARCHAR(80) NOT NULL,
          org VARCHAR(60) NOT NULL,
          plan VARCHAR(20) NOT NULL,
          region VARCHAR(30) NOT NULL,
          country VARCHAR(40) NOT NULL,
          role VARCHAR(20) NOT NULL,
          segment VARCHAR(30) NOT NULL,
          sub_segment VARCHAR(40) NOT NULL,
          industry VARCHAR(40) NOT NULL,
          acquisition_channel VARCHAR(20) NOT NULL,
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
          active_minutes NUMERIC(6,1) NOT NULL,
          org VARCHAR(60) NOT NULL,
          plan VARCHAR(20) NOT NULL,
          region VARCHAR(30) NOT NULL,
          country VARCHAR(40) NOT NULL,
          role VARCHAR(20) NOT NULL,
          segment VARCHAR(30) NOT NULL,
          sub_segment VARCHAR(40) NOT NULL,
          industry VARCHAR(40) NOT NULL,
          acquisition_channel VARCHAR(20) NOT NULL,
          signup_date DATE NOT NULL
        )""", DB)
    t0 = time.time()
    pool.execute_query(USAGE_SQL, DB)
    sys.stderr.write("usage_daily inserted in %.1fs\n" % (time.time() - t0))
    pool.execute_query("CREATE INDEX idx_usage_date ON public.kaveon_usage_daily (usage_date)", DB)
    pool.execute_query("CREATE INDEX idx_usage_user ON public.kaveon_usage_daily (user_id)", DB)
    pool.execute_query("CREATE INDEX idx_usage_country ON public.kaveon_usage_daily (country)", DB)
    pool.execute_query("CREATE INDEX idx_usage_segment ON public.kaveon_usage_daily (segment)", DB)
    pool.execute_query("CREATE INDEX idx_usage_industry ON public.kaveon_usage_daily (industry)", DB)

    stats = pool.execute_query(
        "SELECT COUNT(*), MIN(usage_date), MAX(usage_date), SUM(queries_run), "
        "SUM(dashboards_viewed), COUNT(DISTINCT country), COUNT(DISTINCT industry) "
        "FROM public.kaveon_usage_daily", DB)["rows"][0]
    sys.stderr.write("usage_daily: rows=%s range=%s..%s queries=%s views=%s countries=%s industries=%s\n" % tuple(stats))


if __name__ == "__main__":
    main()
