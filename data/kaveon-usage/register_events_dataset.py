"""Register kaveon_events_enriched as a Kaveon dataset for DLM.
504M-row product analytics: 28 days x 6 surfaces x 3M users.
Idempotent: removes prior 'Kaveon Events' dataset first.
"""
import psycopg2
import sys

DSN = dict(
    host="kaveon-db.postgres.database.azure.com",
    dbname="kaveon",
    user="kaveon_admin",
    password="Kv#bv1r0v_TB=i9NV1YJvMHf7qVW1=nm",
    sslmode="require",
)

VIEW = "kaveon_events_enriched"

COLUMNS = [
    (VIEW, "event_date", "date", False, False),
    (VIEW, "user_id", "integer", False, False),
    (VIEW, "surface", "varchar", True, False),
    (VIEW, "actions", "integer", False, True),
    (VIEW, "sessions", "integer", False, True),
    (VIEW, "duration_sec", "integer", False, True),
    (VIEW, "queries_run", "integer", False, True),
    (VIEW, "charts_created", "integer", False, True),
    (VIEW, "errors", "integer", False, True),
    (VIEW, "rows_scanned", "bigint", False, True),
    (VIEW, "cache_hits", "integer", False, True),
    (VIEW, "latency_p75_ms", "integer", False, True),
    (VIEW, "platform", "varchar", True, False),
    (VIEW, "license", "varchar", True, False),
    (VIEW, "segment", "varchar", True, False),
    (VIEW, "industry", "varchar", True, False),
    (VIEW, "region", "varchar", True, False),
    (VIEW, "country", "varchar", True, False),
    (VIEW, "deployment", "varchar", True, False),
    (VIEW, "acquisition_channel", "varchar", True, False),
    (VIEW, "team_size", "varchar", True, False),
]

METRICS = [
    ("Total Actions", "SUM(actions)", "sum", "#,##0"),
    ("Sessions", "SUM(sessions)", "sum", "#,##0"),
    ("Duration (min)", "SUM(duration_sec) / 60.0", "sum", "#,##0.0"),
    ("Queries Run", "SUM(queries_run)", "sum", "#,##0"),
    ("Charts Created", "SUM(charts_created)", "sum", "#,##0"),
    ("Errors", "SUM(errors)", "sum", "#,##0"),
    ("Rows Scanned", "SUM(rows_scanned)", "sum", "#,##0"),
    ("Cache Hits", "SUM(cache_hits)", "sum", "#,##0"),
    ("Avg Latency (ms)", "AVG(latency_p75_ms)", "avg", "#,##0"),
    ("Active Users", "COUNT(DISTINCT user_id)", "count", "#,##0"),
    ("Error Rate", "CASE WHEN SUM(actions) > 0 THEN SUM(errors)::float / SUM(actions) ELSE 0 END", "ratio", "0.0%"),
    ("Cache Hit Rate", "CASE WHEN SUM(cache_hits) + SUM(actions) > 0 THEN SUM(cache_hits)::float / (SUM(cache_hits) + SUM(actions)) ELSE 0 END", "ratio", "0.0%"),
]


def q(s):
    return s.replace("'", "''")


def run(cur, sql):
    cur.execute(sql)
    if cur.description:
        return cur.fetchall()
    return []


def main():
    conn = psycopg2.connect(**DSN)
    conn.autocommit = True
    cur = conn.cursor()

    run(cur, "DELETE FROM datasets WHERE dataset_name = 'Kaveon Events'")

    desc = (
        "504M-row product analytics for the Kaveon platform — daily per-user "
        "engagement across 6 surfaces (Chat, Dashboard, Chart Builder, SQL Lab, "
        "API, Export) with actions, sessions, duration, queries, errors, cache hits, "
        "and latency. Joined with 3M users across 9 platforms, 4 licenses, "
        "5 segments, 10 industries, 6 regions, 50 countries."
    )
    rows = run(cur,
        "INSERT INTO datasets (dataset_name, description, fact_table, schema_name, "
        "database_name, date_column, visibility, created_by) VALUES "
        "('Kaveon Events', '%s', '%s', 'public', 'kaveon', "
        "'event_date', 'published', 'system') RETURNING id" % (q(desc), VIEW))
    did = rows[0][0]
    print(f"dataset id={did}", file=sys.stderr)

    for tbl, col, dt, is_dim, is_met in COLUMNS:
        run(cur,
            "INSERT INTO dataset_columns (dataset_id, table_name, column_name, "
            "data_type, is_dimension, is_metric) VALUES (%d, '%s', '%s', '%s', %s, %s)"
            % (did, tbl, col, dt, str(is_dim).lower(), str(is_met).lower()))

    for name, expr, mtype, fmt in METRICS:
        run(cur,
            "INSERT INTO dataset_metrics (dataset_id, metric_name, expression, "
            "metric_type, format) VALUES (%d, '%s', '%s', '%s', '%s')"
            % (did, q(name), q(expr), mtype, q(fmt)))

    nc = run(cur, "SELECT COUNT(*) FROM dataset_columns WHERE dataset_id=%d" % did)[0][0]
    nm = run(cur, "SELECT COUNT(*) FROM dataset_metrics WHERE dataset_id=%d" % did)[0][0]
    print(f"registered: id={did} columns={nc} metrics={nm}", file=sys.stderr)
    print(did)

    conn.close()


if __name__ == "__main__":
    main()
