"""
Build kaveon_events_daily (504M rows) via COPY FROM STDIN.
Run directly: python scripts/build_504m.py
Expected: ~78 min on a fast machine, UNLOGGED table on B1ms.
"""
import psycopg2
import numpy as np
import io
import time
import sys

DSN = dict(
    host="kaveon-db.postgres.database.azure.com",
    dbname="kaveon",
    user="kaveon_admin",
    password="Kv#bv1r0v_TB=i9NV1YJvMHf7qVW1=nm",
    sslmode="require",
    options="-c statement_timeout=0",
)

N = 3_000_000
UIDS = np.arange(1, N + 1, dtype=np.int64)

SURFACES = [
    (1, "Chat",          (3,15),  (1,4), (60,1800),   (0,3),  (0,0), (0,1), (0,1000),       (0,2),  (100,500)),
    (2, "Dashboard",     (5,25),  (1,5), (120,3600),  (2,10), (0,1), (0,2), (1000,100000),   (2,8),  (200,1500)),
    (3, "Chart Builder", (3,12),  (1,3), (180,2400),  (3,15), (1,5), (0,2), (5000,500000),   (1,5),  (300,2000)),
    (4, "SQL Lab",       (5,20),  (1,4), (300,3600),  (5,25), (0,1), (0,3), (10000,1000000), (0,3),  (500,3000)),
    (5, "API",           (10,50), (1,2), (30,600),    (1,5),  (0,0), (0,1), (100,50000),     (3,10), (50,500)),
    (6, "Export",        (1,5),   (1,2), (30,300),    (1,3),  (0,0), (0,1), (50000,1000000), (0,2),  (200,1000)),
]

NAMES = {1:"Chat", 2:"Dashboard", 3:"Chart Builder", 4:"SQL Lab", 5:"API", 6:"Export"}

def hcol(seed, lo, hi):
    rng = hi - lo + 1
    if rng <= 0:
        return np.full(N, lo, dtype=np.int64)
    return lo + np.abs((UIDS * seed) % rng)

def gen_batch(day_off, sc, ranges):
    act_r, sess_r, dur_r, qr_r, ch_r, err_r, rs_r, cache_r, lat_r = ranges
    data = np.column_stack([
        hcol(2654435761 + sc*40503 + day_off*12289 + 7, rs_r[0], rs_r[1]),
        UIDS,
        hcol(1300813 + sc*997 + day_off*251 + 5, dur_r[0], dur_r[1]),
        np.full(N, sc, dtype=np.int64),
        hcol(486187 + sc*31 + day_off*127 + 1, act_r[0], act_r[1]),
        hcol(999331 + sc*53 + day_off*193 + 2, sess_r[0], sess_r[1]),
        hcol(735391 + sc*71 + day_off*311 + 3, qr_r[0], qr_r[1]),
        hcol(571373 + sc*97 + day_off*409 + 4, ch_r[0], ch_r[1]),
        hcol(412619 + sc*113 + day_off*503 + 6, err_r[0], err_r[1]),
        hcol(297179 + sc*137 + day_off*601 + 8, cache_r[0], cache_r[1]),
        hcol(193939 + sc*151 + day_off*701 + 9, lat_r[0], lat_r[1]),
    ])
    buf = io.BytesIO()
    fmt = f"%d\t2026-07-{4+day_off:02d}\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d"
    np.savetxt(buf, data, fmt=fmt, delimiter="")
    buf.seek(0)
    return buf

def main():
    conn = psycopg2.connect(**DSN)
    conn.autocommit = True
    cur = conn.cursor()

    cur.execute("SELECT count(*) FROM kaveon_users")
    uc = cur.fetchone()[0]
    print(f"kaveon_users: {uc:,}")
    if uc != N:
        print(f"ERROR: expected {N:,} users, got {uc:,}")
        sys.exit(1)

    print("\nDropping + creating UNLOGGED kaveon_events_daily...")
    cur.execute("DROP TABLE IF EXISTS kaveon_events_daily CASCADE")
    cur.execute("""
    CREATE UNLOGGED TABLE kaveon_events_daily (
        rows_scanned   BIGINT   NOT NULL,
        event_date     DATE     NOT NULL,
        user_id        INT      NOT NULL,
        duration_sec   INT      NOT NULL,
        surface        SMALLINT NOT NULL,
        actions        SMALLINT NOT NULL,
        sessions       SMALLINT NOT NULL,
        queries_run    SMALLINT NOT NULL,
        charts_created SMALLINT NOT NULL,
        errors         SMALLINT NOT NULL,
        cache_hits     SMALLINT NOT NULL,
        latency_p75_ms SMALLINT NOT NULL
    )
    """)
    cur.execute("ALTER TABLE kaveon_events_daily SET (autovacuum_enabled = false)")
    print("  done\n")

    total_rows = 0
    total_time = 0

    for day_off in range(28):
        day_start = time.time()
        day = f"2026-07-{4+day_off:02d}"
        print(f"Day {day_off+1}/28: {day}")

        for sc, name, *ranges in SURFACES:
            t0 = time.time()
            buf = gen_batch(day_off, sc, ranges)
            gen_dt = time.time() - t0
            sz = buf.getbuffer().nbytes

            t1 = time.time()
            cur.copy_expert("COPY kaveon_events_daily FROM STDIN", buf)
            copy_dt = time.time() - t1

            total_rows += N
            dt = time.time() - t0
            mbs = sz / copy_dt / 1e6 if copy_dt > 0 else 0
            print(f"  {name:14s} {dt:5.1f}s (gen={gen_dt:.1f} copy={copy_dt:.1f} {sz/1e6:.0f}MB @{mbs:.0f}MB/s) [{total_rows/1e6:.0f}M]")

        day_dt = time.time() - day_start
        total_time += day_dt
        eta = (total_time / (day_off+1)) * (27 - day_off)
        print(f"  -> {day_dt:.0f}s  ETA: {eta/60:.0f}min\n")

    print("Creating VIEW...")
    cur.execute("DROP VIEW IF EXISTS kaveon_events_enriched CASCADE")
    cur.execute("""
    CREATE VIEW kaveon_events_enriched AS
    SELECT
        e.event_date, e.user_id,
        CASE e.surface
            WHEN 1 THEN 'Chat' WHEN 2 THEN 'Dashboard'
            WHEN 3 THEN 'Chart Builder' WHEN 4 THEN 'SQL Lab'
            WHEN 5 THEN 'API' WHEN 6 THEN 'Export'
        END AS surface,
        e.actions, e.sessions, e.duration_sec, e.queries_run,
        e.charts_created, e.errors, e.rows_scanned, e.cache_hits, e.latency_p75_ms,
        u.platform, u.license, u.segment, u.industry, u.region, u.country,
        u.deployment, u.acquisition_channel, u.team_size
    FROM kaveon_events_daily e
    JOIN kaveon_users u ON u.user_id = e.user_id
    """)
    print("  done")

    print("\nRe-enabling autovacuum + ANALYZE...")
    cur.execute("ALTER TABLE kaveon_events_daily SET (autovacuum_enabled = true)")
    cur.execute("ANALYZE kaveon_users")
    print("  done (skip ANALYZE on events — too slow on B1ms, will autovacuum later)")

    print(f"\n=== COMPLETE ===")
    cur.execute("SELECT pg_size_pretty(pg_total_relation_size('kaveon_events_daily'))")
    print(f"  events: {cur.fetchone()[0]}")
    cur.execute("SELECT pg_size_pretty(pg_database_size(current_database()))")
    print(f"  database: {cur.fetchone()[0]}")
    print(f"  rows: {total_rows:,}")
    print(f"  time: {total_time/60:.1f} min")

    conn.close()

if __name__ == "__main__":
    main()
