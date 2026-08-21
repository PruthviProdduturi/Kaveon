"""Generate L1 event-level data from L2 kaveon_usage_daily.

Decomposes each user-day row into individual events:
  - queries_run      → event_type=1 ('query')
  - nl_queries       → event_type=2 ('nl_query')
  - sql_lab_runs     → event_type=3 ('sql_lab')
  - dashboards_viewed→ event_type=4 ('dashboard_view')
  - charts_created   → event_type=5 ('chart_create')
  - datasets_accessed→ event_type=6 ('dataset_access')
  - exports          → event_type=7 ('export')

Target: ~500M rows on public.kaveon_events + view kaveon_events_full.
Strategy: batch inserts (50 rounds × ~10M = ~500M), each round expands
every user-day with generate_series(1, N) events at random timestamps.

Run from apps/kaveon-api:  python ../../data/kaveon-usage/generate_events.py
"""
import os
import sys
import time

_API = os.path.join(os.path.dirname(__file__), "..", "..", "apps", "kaveon-api")
sys.path.insert(0, os.path.abspath(_API))

import database.pool as pool  # noqa: E402

DB = "kaveon"
BATCH_SIZE = 5
NUM_BATCHES = 10
# Each batch: 10.12M user-days × BATCH_SIZE events ≈ 50M rows
# Total: NUM_BATCHES × 50M = ~500M rows


def main():
    pool.execute_query("SELECT 1", DB)

    pool.execute_query("DROP TABLE IF EXISTS public.kaveon_events", DB)
    pool.execute_query("DROP VIEW IF EXISTS public.kaveon_events_full", DB)

    pool.execute_query("""
        CREATE TABLE public.kaveon_events (
          event_ts  TIMESTAMP NOT NULL,
          user_id   INT NOT NULL,
          event_type SMALLINT NOT NULL,
          duration_sec REAL NOT NULL
        )""", DB)

    total_rows = 0
    t_start = time.time()

    for batch in range(NUM_BATCHES):
        t0 = time.time()
        pool.execute_query(f"""
            INSERT INTO public.kaveon_events (event_ts, user_id, event_type, duration_sec)
            SELECT
              usage_date + ((random() * 86400)::int * interval '1 second'),
              user_id,
              1 + floor(random()*7 + s.n*0)::int,
              round((random() * 300)::numeric, 1)
            FROM public.kaveon_usage_daily, generate_series(1, {BATCH_SIZE}) s(n)
        """, DB)
        elapsed = time.time() - t0
        total_rows += 10120000 * BATCH_SIZE
        sys.stderr.write(
            "  batch %d/%d: +%dM rows in %.0fs (total: %dM, %.0fs elapsed)\n"
            % (batch + 1, NUM_BATCHES, 10120000 * BATCH_SIZE // 1_000_000,
               elapsed, total_rows // 1_000_000, time.time() - t_start))

    sys.stderr.write("\nCreating indexes...\n")
    pool.execute_query(
        "CREATE INDEX idx_events_user ON public.kaveon_events (user_id)", DB)
    pool.execute_query(
        "CREATE INDEX idx_events_ts ON public.kaveon_events (event_ts)", DB)

    sys.stderr.write("Creating view kaveon_events_full...\n")
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
          u.org, u.plan, u.region, u.country, u.role,
          u.segment, u.sub_segment, u.industry, u.acquisition_channel
        FROM public.kaveon_events e
        JOIN public.kaveon_users u ON e.user_id = u.user_id
    """, DB)

    stats = pool.execute_query(
        "SELECT COUNT(*), MIN(event_ts), MAX(event_ts) FROM public.kaveon_events", DB)["rows"][0]
    sys.stderr.write(
        "\nDone: kaveon_events: %s rows, range %s .. %s (%.1f min total)\n"
        % (stats[0], stats[1], stats[2], (time.time() - t_start) / 60))


if __name__ == "__main__":
    main()
