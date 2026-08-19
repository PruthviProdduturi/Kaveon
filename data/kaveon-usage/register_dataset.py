"""Register the synthetic Kaveon usage tables as a queryable Kaveon dataset
(datasets + dataset_dimensions + dataset_columns + dataset_metrics), then the
DLM can compile it. Idempotent: removes any prior 'Kaveon Product Usage' dataset.

Run from apps/kaveon-api:  python ../../data/kaveon-usage/register_dataset.py
"""
import os
import sys

_API = os.path.join(os.path.dirname(__file__), "..", "..", "apps", "kaveon-api")
sys.path.insert(0, os.path.abspath(_API))

import database.pool as pool  # noqa: E402

DB = "kaveon"

# (table, column, data_type, is_dimension, is_metric)
COLUMNS = [
    ("kaveon_usage_daily", "usage_date", "date", False, False),
    ("kaveon_usage_daily", "user_id", "integer", False, False),
    ("kaveon_usage_daily", "queries_run", "integer", False, True),
    ("kaveon_usage_daily", "nl_queries", "integer", False, True),
    ("kaveon_usage_daily", "sql_lab_runs", "integer", False, True),
    ("kaveon_usage_daily", "dashboards_viewed", "integer", False, True),
    ("kaveon_usage_daily", "charts_created", "integer", False, True),
    ("kaveon_usage_daily", "datasets_accessed", "integer", False, True),
    ("kaveon_usage_daily", "exports", "integer", False, True),
    ("kaveon_usage_daily", "active_minutes", "numeric", False, True),
    ("kaveon_usage_daily", "org", "varchar", True, False),
    ("kaveon_usage_daily", "plan", "varchar", True, False),
    ("kaveon_usage_daily", "region", "varchar", True, False),
    ("kaveon_usage_daily", "role", "varchar", True, False),
    ("kaveon_usage_daily", "signup_date", "date", False, False),
]

# (name, expression, type, format)
METRICS = [
    ("Total Queries", "SUM(queries_run)", "sum", "#,##0"),
    ("NL Queries", "SUM(nl_queries)", "sum", "#,##0"),
    ("SQL Lab Runs", "SUM(sql_lab_runs)", "sum", "#,##0"),
    ("Dashboard Views", "SUM(dashboards_viewed)", "sum", "#,##0"),
    ("Charts Created", "SUM(charts_created)", "sum", "#,##0"),
    ("Datasets Accessed", "SUM(datasets_accessed)", "sum", "#,##0"),
    ("Total Exports", "SUM(exports)", "sum", "#,##0"),
    ("Avg Active Minutes", "AVG(active_minutes)", "avg", "#,##0.0"),
    ("Active Users", "COUNT(DISTINCT user_id)", "count", "#,##0"),
]


def q(s):
    return s.replace("'", "''")


def main():
    pool.execute_query("SELECT 1", DB)
    # remove prior registration (cascades to dims/cols/metrics via FK ON DELETE CASCADE)
    pool.execute_query("DELETE FROM datasets WHERE dataset_name = 'Kaveon Product Usage'", DB)

    desc = ("Daily per-user product usage of the Kaveon platform — queries run, "
            "NL vs SQL Lab, dashboards viewed, charts created, exports and active "
            "minutes, by plan, region, role and org. Synthetic.")
    ins = pool.execute_query(
        "INSERT INTO datasets (dataset_name, description, fact_table, schema_name, "
        "database_name, date_column, visibility, created_by) VALUES "
        "('Kaveon Product Usage', '%s', 'kaveon_usage_daily', 'public', 'kaveon', "
        "'usage_date', 'published', 'system') RETURNING id" % q(desc), DB)
    did = ins["rows"][0][0]
    sys.stderr.write("dataset id=%s\n" % did)

    # Dims are denormalized onto the fact table (flat/wide) — no join dimension,
    # matching how the DLM assembler queries (single fact table).
    for tbl, col, dt, is_dim, is_met in COLUMNS:
        pool.execute_query(
            "INSERT INTO dataset_columns (dataset_id, table_name, column_name, "
            "data_type, is_dimension, is_metric) VALUES (%d, '%s', '%s', '%s', %s, %s)"
            % (did, tbl, col, dt, str(is_dim).lower(), str(is_met).lower()), DB)

    for name, expr, mtype, fmt in METRICS:
        pool.execute_query(
            "INSERT INTO dataset_metrics (dataset_id, metric_name, expression, "
            "metric_type, format) VALUES (%d, '%s', '%s', '%s', '%s')"
            % (did, q(name), q(expr), mtype, q(fmt)), DB)

    nc = pool.execute_query("SELECT COUNT(*) FROM dataset_columns WHERE dataset_id=%d" % did, DB)["rows"][0][0]
    nm = pool.execute_query("SELECT COUNT(*) FROM dataset_metrics WHERE dataset_id=%d" % did, DB)["rows"][0][0]
    sys.stderr.write("registered: id=%s columns=%s metrics=%s\n" % (did, nc, nm))
    print(did)


if __name__ == "__main__":
    main()
