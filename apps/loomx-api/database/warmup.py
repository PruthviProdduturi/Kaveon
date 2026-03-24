"""
Connection pool warmup and heartbeat.
Direct port of _run_warmup_and_heartbeat() from the Flask proxy.

Flow:
  1. Warm metadata DB first (6 connections) — endpoint details live here.
  2. Discover active data sources from data_sources table.
  3. Warm each data source pool (1 connection each) in parallel threads.
  4. Enter heartbeat loop: ping all pools every 5 minutes.
"""

import time
import threading
from typing import List

from config import settings
from database.pool import (
    _pools,
    get_connection_pool,
    is_connection_error,
)


def _warm_pool(db_name: str, n_conns: int, label: str) -> bool:
    if not db_name:
        return False
    delays = [0, 10, 20]
    for attempt, delay in enumerate(delays, 1):
        if delay:
            time.sleep(delay)
        conns = []
        try:
            print(f"[Warmup] Warming {label} ({db_name}) — attempt {attempt}/3…")
            pool = get_connection_pool(db_name)
            for _ in range(n_conns):
                conn = pool.get_connection()
                conn.execute_query("SELECT 1 AS warmup")
                conns.append((pool, conn))
            print(f"[Warmup] {label} ready ({n_conns} connection(s) warmed).")
            return True
        except Exception as e:
            print(f"[Warmup] {label} attempt {attempt} failed: {e}")
            if attempt < len(delays):
                print(f"[Warmup] Retrying {label} in {delays[attempt]}s…")
        finally:
            for pool, conn in conns:
                pool.return_connection(conn)
    print(f"[Warmup] {label} gave up — first request will connect lazily.")
    return False


def start_warmup_and_heartbeat():
    """
    Entry point — run in a daemon thread from main.py lifespan startup.
    Warms all pools, then runs a heartbeat loop forever.
    """
    from database.pool import _live_meta_db
    meta_db = _live_meta_db()
    meta_ok = _warm_pool(meta_db, 6, "metadata")

    if not meta_ok:
        print("[Warmup] Metadata unavailable — cannot discover other data sources.")
        return

    # Discover active data source databases from metadata
    active_dbs: List[str] = []
    try:
        pool = get_connection_pool(meta_db)
        conn = pool.get_connection()
        try:
            result = conn.execute_query(
                "SELECT DISTINCT database_name FROM data_sources "
                "WHERE is_active = 1 AND database_name IS NOT NULL"
            )
            active_dbs = [r["database_name"] for r in (result.get("rows_objects") or [])]
            print(f"[Warmup] Discovered {len(active_dbs)} active data source(s): {active_dbs}")
        finally:
            pool.return_connection(conn)
    except Exception as e:
        print(f"[Warmup] Could not query data_sources: {e}")

    # Fallback: also warm the default warehouse if set and not already covered
    dw_db = settings.DATAWAREHOUSE_DATABASE
    if dw_db and dw_db not in active_dbs:
        active_dbs.append(dw_db)

    # Warm each data source in parallel
    threads = []
    for db_name in active_dbs:
        t = threading.Thread(
            target=_warm_pool,
            args=(db_name, 1, f"data-source({db_name})"),
            daemon=True,
        )
        t.start()
        threads.append(t)
    for t in threads:
        t.join()

    # Heartbeat: keep all pools alive every 5 minutes
    heartbeat_dbs = [meta_db] + active_dbs
    while True:
        time.sleep(300)
        for db in heartbeat_dbs:
            if not db:
                continue
            hb_pool = None
            hb_conn = None
            try:
                hb_pool = get_connection_pool(db)
                hb_conn = hb_pool.get_connection()
                hb_conn.execute_query("SELECT 1 AS heartbeat")
            except Exception as e:
                print(f"[Heartbeat] Failed for {db}: {e}")
                if hb_conn and is_connection_error(e):
                    if hb_pool:
                        hb_pool.discard_connection(hb_conn)
                    hb_conn = None
            finally:
                if hb_conn and hb_pool:
                    hb_pool.return_connection(hb_conn)
