"""Diagnose day-vs-month time-series: what SQL + columns the builder produces."""
import os
import json
import psycopg2
import requests

NEON = os.environ["NEON_URL"]
API = "http://localhost:8080"
H = {"x-user-email": "pruthvi.prodduturi@gmail.com", "x-user-role": "Admin",
     "x-user-roles": "Admin", "x-proxy-secret": "dev-only-proxy-secret-change-me-fedcba9876543210",
     "content-type": "application/json"}

c = psycopg2.connect(NEON); cur = c.cursor()
cur.execute("select id from datasets where fact_table='covid_daily' order by id desc limit 1")
did = cur.fetchone()[0]
cur.close(); c.close()
print("daily dataset id:", did)

for grain in ("day", "month"):
    cfg = {"metrics": [{"column": "new_confirmed", "aggregate": "SUM", "label": "New Cases"}],
           "time_column": "dt", "time_grain": grain, "time_range": "all_time",
           "query_mode": "aggregate", "row_limit": 5000}
    g = requests.post(f"{API}/api/v1/sql/generate", headers=H,
                      json={"dataset_id": did, "chart_type": "time_series_line", "config": cfg}, timeout=30)
    if not g.ok:
        print(grain, "GEN FAIL", g.status_code, g.text[:200]); continue
    sql = g.json()["sql_text"]
    e = requests.post(f"{API}/api/v1/sql/execute", headers=H,
                      json={"sql_text": sql, "database": "neondb", "source": "chart-builder"}, timeout=60)
    cols = e.json().get("columns") if e.ok else e.text[:200]
    n = len(e.json().get("rows", [])) if e.ok else 0
    print(f"\n== {grain} ==\nSQL: {sql}\ncolumns: {cols}  rows: {n}")
