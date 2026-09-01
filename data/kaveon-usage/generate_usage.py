"""Generate Kaveon product-usage star schema.

Dimension tables:
  public.dim_geography     — locale → country → region  (26 rows)
  public.dim_platform      — platform_key → platform, os  (9 rows)
  public.kaveon_users      — user dimension  (44K rows)

Fact tables:
  public.kaveon_usage_daily — L2 daily aggregated  (~10M rows)

The L1 event table (public.kaveon_events, ~506M rows) is generated
separately by generate_events.py and kept intact across rebuilds.

VIEWs:
  public.kaveon_product_analytics — L2 fact + all dims joined
  public.kaveon_events_full       — L1 fact + all dims joined

Run from repo root:  python data/kaveon-usage/generate_usage.py
"""
import os
import sys
import time

_API = os.path.join(os.path.dirname(__file__), "..", "..", "api")
sys.path.insert(0, os.path.abspath(_API))

import database.pool as pool  # noqa: E402

DB = "kaveon"

# ── Dimension: Geography ─────────────────────────────────────────────────────

GEO_ROWS = [
    ("en-US", "United States", "North America"),
    ("en-CA", "Canada", "North America"),
    ("es-MX", "Mexico", "North America"),
    ("en-GB", "United Kingdom", "Europe"),
    ("de-DE", "Germany", "Europe"),
    ("fr-FR", "France", "Europe"),
    ("nl-NL", "Netherlands", "Europe"),
    ("es-ES", "Spain", "Europe"),
    ("sv-SE", "Sweden", "Europe"),
    ("it-IT", "Italy", "Europe"),
    ("hi-IN", "India", "Asia"),
    ("ja-JP", "Japan", "Asia"),
    ("en-SG", "Singapore", "Asia"),
    ("ko-KR", "South Korea", "Asia"),
    ("id-ID", "Indonesia", "Asia"),
    ("zh-CN", "China", "Asia"),
    ("pt-BR", "Brazil", "South America"),
    ("es-AR", "Argentina", "South America"),
    ("es-CO", "Colombia", "South America"),
    ("es-CL", "Chile", "South America"),
    ("en-NG", "Nigeria", "Africa"),
    ("en-ZA", "South Africa", "Africa"),
    ("sw-KE", "Kenya", "Africa"),
    ("ar-EG", "Egypt", "Africa"),
    ("en-AU", "Australia", "Oceania"),
    ("en-NZ", "New Zealand", "Oceania"),
]

# ── Dimension: Platform ──────────────────────────────────────────────────────

PLATFORM_ROWS = [
    ("desktop-win", "Desktop", "Windows"),
    ("desktop-mac", "Desktop", "macOS"),
    ("desktop-linux", "Desktop", "Linux"),
    ("mobile-ios", "Mobile", "iOS"),
    ("mobile-android", "Mobile", "Android"),
    ("web-win", "Web", "Windows"),
    ("web-mac", "Web", "macOS"),
    ("web-linux", "Web", "Linux"),
    ("web-chromeos", "Web", "ChromeOS"),
]

# ── Pools for random assignment ──────────────────────────────────────────────
# Weighted pools — duplicates increase probability.

_LOCALE_POOL = (
    "en-US,en-US,en-US,en-US,en-CA,es-MX,"
    "en-GB,en-GB,de-DE,fr-FR,nl-NL,es-ES,sv-SE,it-IT,"
    "hi-IN,hi-IN,ja-JP,en-SG,ko-KR,id-ID,zh-CN,"
    "pt-BR,es-AR,es-CO,es-CL,"
    "en-NG,en-ZA,sw-KE,ar-EG,"
    "en-AU,en-NZ"
)
_LOCALE_N = _LOCALE_POOL.count(",") + 1

_PLAT_POOL = (
    "desktop-win,desktop-win,desktop-win,desktop-mac,desktop-mac,desktop-linux,"
    "mobile-ios,mobile-ios,mobile-android,mobile-android,mobile-android,"
    "web-win,web-win,web-mac,web-linux,web-chromeos"
)
_PLAT_N = _PLAT_POOL.count(",") + 1

_LICENSE_POOL = "Free,Free,Free,Standard,Standard,Professional,Professional,Enterprise"
_LICENSE_N = _LICENSE_POOL.count(",") + 1

_AUDIENCE_POOL = "Consumer,Consumer,Consumer,Commercial,Commercial,Commercial,Education,Government"
_AUDIENCE_N = _AUDIENCE_POOL.count(",") + 1

_SEGMENT_POOL = "Enterprise,Enterprise,Mid-Market,Mid-Market,Mid-Market,SMB,SMB,Startup,Startup"
_SEGMENT_N = _SEGMENT_POOL.count(",") + 1

_INDUSTRY_POOL = (
    "Technology,Technology,Technology,Healthcare,Financial Services,Manufacturing,"
    "Retail,Education,Media,Energy,Government,Logistics,Real Estate,Professional Services"
)
_INDUSTRY_N = _INDUSTRY_POOL.count(",") + 1

_TEAM_POOL = "Solo,Solo,Small,Small,Small,Medium,Medium,Large,Enterprise"
_TEAM_N = _TEAM_POOL.count(",") + 1

_DEPLOY_POOL = "Cloud,Cloud,Cloud,Cloud,Hybrid,On-Premise"
_DEPLOY_N = _DEPLOY_POOL.count(",") + 1

_ACQ_POOL = "Organic,Organic,Organic,Referral,Referral,Partner,Paid,Paid,Direct"
_ACQ_N = _ACQ_POOL.count(",") + 1


USERS_SQL = f"""
INSERT INTO public.kaveon_users
SELECT g,
  locale, platform_key, license, audience, segment, industry,
  team_size, deployment, acq,
  (date '2024-01-01' + (random() * 940)::int)
FROM (
  SELECT g,
    split_part('{_LOCALE_POOL}', ',', 1 + floor(random()*{_LOCALE_N} + g*0)::int) AS locale,
    split_part('{_PLAT_POOL}', ',', 1 + floor(random()*{_PLAT_N} + g*0)::int) AS platform_key,
    split_part('{_LICENSE_POOL}', ',', 1 + floor(random()*{_LICENSE_N} + g*0)::int) AS license,
    split_part('{_AUDIENCE_POOL}', ',', 1 + floor(random()*{_AUDIENCE_N} + g*0)::int) AS audience,
    split_part('{_SEGMENT_POOL}', ',', 1 + floor(random()*{_SEGMENT_N} + g*0)::int) AS segment,
    split_part('{_INDUSTRY_POOL}', ',', 1 + floor(random()*{_INDUSTRY_N} + g*0)::int) AS industry,
    split_part('{_TEAM_POOL}', ',', 1 + floor(random()*{_TEAM_N} + g*0)::int) AS team_size,
    split_part('{_DEPLOY_POOL}', ',', 1 + floor(random()*{_DEPLOY_N} + g*0)::int) AS deployment,
    split_part('{_ACQ_POOL}', ',', 1 + floor(random()*{_ACQ_N} + g*0)::int) AS acq
  FROM generate_series(1, 44000) g
) t
"""

# L2 fact: daily per-user metrics. License weight drives metric volume.
USAGE_SQL = """
INSERT INTO public.kaveon_usage_daily
  (usage_date, user_id, queries_run, nl_queries, sql_lab_runs,
   dashboards_viewed, charts_created, datasets_accessed, exports,
   active_minutes, sessions, api_calls, data_processed_mb,
   errors, feedback_positive, feedback_negative)
SELECT
  ud AS usage_date,
  u.user_id,
  q AS queries_run,
  round(q * (0.30 + random()*0.40))::int AS nl_queries,
  round(q * (0.20 + random()*0.30))::int AS sql_lab_runs,
  least(q, greatest(0, round(lw * wf * (0.3 + random()*2.0))::int)) AS dashboards_viewed,
  greatest(0, round(lw * wf * random()*1.5)::int) AS charts_created,
  greatest(0, round(lw * wf * (1 + random()*3))::int) AS datasets_accessed,
  greatest(0, round(lw * wf * random()*1.2)::int) AS exports,
  round((lw * wf * (5 + random()*40))::numeric, 1) AS active_minutes,
  greatest(1, round(lw * wf * (1 + random()*4))::int) AS sessions,
  greatest(0, round(lw * wf * random()*15)::int) AS api_calls,
  round((lw * wf * (0.1 + random()*50))::numeric, 1) AS data_processed_mb,
  CASE WHEN random() < 0.05 THEN greatest(0, round(random()*3)::int) ELSE 0 END AS errors,
  CASE WHEN random() < 0.15 THEN 1 ELSE 0 END AS feedback_positive,
  CASE WHEN random() < 0.03 THEN 1 ELSE 0 END AS feedback_negative
FROM public.kaveon_users u
CROSS JOIN LATERAL generate_series(0, 229) AS s(day_off)
CROSS JOIN LATERAL (SELECT (date '2026-01-01' + s.day_off) AS ud) dd
CROSS JOIN LATERAL (
  SELECT
    (CASE u.license
       WHEN 'Enterprise' THEN 8 WHEN 'Professional' THEN 4
       WHEN 'Standard' THEN 2 ELSE 1 END)::numeric
      * (0.7 + 0.6 * (s.day_off / 229.0)) AS lw,
    (CASE WHEN extract(dow FROM (date '2026-01-01' + s.day_off)) IN (0, 6)
          THEN 0.35 ELSE 1.0 END)::numeric AS wf
) w
CROSS JOIN LATERAL (
  SELECT greatest(0, round(lw * wf * (0.5 + random()*3.5))::int) AS q
) qq
"""


def main():
    pool.execute_query("SELECT 1", DB)

    # ── Drop in dependency order ─────────────────────────────────────────────
    pool.execute_query("DROP VIEW IF EXISTS public.kaveon_product_analytics", DB)
    pool.execute_query("DROP VIEW IF EXISTS public.kaveon_events_full", DB)
    pool.execute_query("DROP TABLE IF EXISTS public.kaveon_usage_daily", DB)
    pool.execute_query("DROP TABLE IF EXISTS public.kaveon_users", DB)
    pool.execute_query("DROP TABLE IF EXISTS public.dim_geography", DB)
    pool.execute_query("DROP TABLE IF EXISTS public.dim_platform", DB)

    # ── Dimension: Geography ─────────────────────────────────────────────────
    pool.execute_query("""
        CREATE TABLE public.dim_geography (
          locale VARCHAR(10) PRIMARY KEY,
          country VARCHAR(40) NOT NULL,
          region VARCHAR(30) NOT NULL
        )""", DB)
    for locale, country, region in GEO_ROWS:
        pool.execute_query(
            "INSERT INTO public.dim_geography VALUES (%s, %s, %s)",
            DB, [locale, country, region])
    sys.stderr.write("dim_geography: %d rows\n" % len(GEO_ROWS))

    # ── Dimension: Platform ──────────────────────────────────────────────────
    pool.execute_query("""
        CREATE TABLE public.dim_platform (
          platform_key VARCHAR(20) PRIMARY KEY,
          platform VARCHAR(10) NOT NULL,
          os VARCHAR(15) NOT NULL
        )""", DB)
    for pk, platform, os_name in PLATFORM_ROWS:
        pool.execute_query(
            "INSERT INTO public.dim_platform VALUES (%s, %s, %s)",
            DB, [pk, platform, os_name])
    sys.stderr.write("dim_platform: %d rows\n" % len(PLATFORM_ROWS))

    # ── User dimension ───────────────────────────────────────────────────────
    pool.execute_query("""
        CREATE TABLE public.kaveon_users (
          user_id INT PRIMARY KEY,
          locale VARCHAR(10) NOT NULL REFERENCES public.dim_geography(locale),
          platform_key VARCHAR(20) NOT NULL REFERENCES public.dim_platform(platform_key),
          license VARCHAR(20) NOT NULL,
          audience VARCHAR(20) NOT NULL,
          segment VARCHAR(20) NOT NULL,
          industry VARCHAR(40) NOT NULL,
          team_size VARCHAR(20) NOT NULL,
          deployment VARCHAR(15) NOT NULL,
          acquisition_channel VARCHAR(20) NOT NULL,
          signup_date DATE NOT NULL
        )""", DB)
    pool.execute_query(USERS_SQL, DB)
    n_users = pool.execute_query("SELECT COUNT(*) FROM public.kaveon_users", DB)["rows"][0][0]
    sys.stderr.write("kaveon_users: %s rows\n" % n_users)

    # ── L2 Fact: daily usage ─────────────────────────────────────────────────
    pool.execute_query("""
        CREATE TABLE public.kaveon_usage_daily (
          id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
          usage_date DATE NOT NULL,
          user_id INT NOT NULL REFERENCES public.kaveon_users(user_id),
          queries_run INT NOT NULL,
          nl_queries INT NOT NULL,
          sql_lab_runs INT NOT NULL,
          dashboards_viewed INT NOT NULL,
          charts_created INT NOT NULL,
          datasets_accessed INT NOT NULL,
          exports INT NOT NULL,
          active_minutes NUMERIC(6,1) NOT NULL,
          sessions INT NOT NULL,
          api_calls INT NOT NULL,
          data_processed_mb NUMERIC(8,1) NOT NULL,
          errors INT NOT NULL,
          feedback_positive INT NOT NULL,
          feedback_negative INT NOT NULL
        )""", DB)
    t0 = time.time()
    pool.execute_query(USAGE_SQL, DB)
    sys.stderr.write("kaveon_usage_daily inserted in %.1fs\n" % (time.time() - t0))

    pool.execute_query("CREATE INDEX idx_usage_date ON public.kaveon_usage_daily (usage_date)", DB)
    pool.execute_query("CREATE INDEX idx_usage_user ON public.kaveon_usage_daily (user_id)", DB)

    # ── VIEWs ────────────────────────────────────────────────────────────────
    pool.execute_query("""
        CREATE VIEW public.kaveon_product_analytics AS
        SELECT
          f.usage_date, f.user_id,
          f.queries_run, f.nl_queries, f.sql_lab_runs, f.dashboards_viewed,
          f.charts_created, f.datasets_accessed, f.exports, f.active_minutes,
          f.sessions, f.api_calls, f.data_processed_mb,
          f.errors, f.feedback_positive, f.feedback_negative,
          u.license, u.audience, u.segment, u.industry,
          u.team_size, u.deployment, u.acquisition_channel,
          u.locale, g.country, g.region,
          u.platform_key, p.platform, p.os
        FROM public.kaveon_usage_daily f
        JOIN public.kaveon_users u ON f.user_id = u.user_id
        JOIN public.dim_geography g ON u.locale = g.locale
        JOIN public.dim_platform p ON u.platform_key = p.platform_key
    """, DB)

    pool.execute_query("""
        CREATE VIEW public.kaveon_events_full AS
        SELECT
          e.event_ts, e.user_id, e.event_type,
          CASE e.event_type
            WHEN 1 THEN 'query' WHEN 2 THEN 'nl_query' WHEN 3 THEN 'sql_lab'
            WHEN 4 THEN 'dashboard_view' WHEN 5 THEN 'chart_create'
            WHEN 6 THEN 'dataset_access' WHEN 7 THEN 'export'
          END AS event_name,
          e.duration_sec,
          e.event_ts::date AS event_date,
          u.license, u.audience, u.segment, u.industry,
          u.team_size, u.deployment, u.acquisition_channel,
          u.locale, g.country, g.region,
          u.platform_key, p.platform, p.os
        FROM public.kaveon_events e
        JOIN public.kaveon_users u ON e.user_id = u.user_id
        JOIN public.dim_geography g ON u.locale = g.locale
        JOIN public.dim_platform p ON u.platform_key = p.platform_key
    """, DB)

    # ── Stats ────────────────────────────────────────────────────────────────
    stats = pool.execute_query(
        "SELECT COUNT(*), MIN(usage_date), MAX(usage_date), SUM(queries_run), "
        "SUM(dashboards_viewed), COUNT(DISTINCT u.locale) "
        "FROM public.kaveon_usage_daily f JOIN public.kaveon_users u ON f.user_id = u.user_id",
        DB)["rows"][0]
    sys.stderr.write(
        "L2: rows=%s range=%s..%s queries=%s views=%s locales=%s\n" % tuple(stats))

    events = pool.execute_query(
        "SELECT COUNT(*) FROM public.kaveon_events", DB)["rows"][0][0]
    sys.stderr.write("L1: kaveon_events=%s rows (unchanged)\n" % events)


if __name__ == "__main__":
    main()
