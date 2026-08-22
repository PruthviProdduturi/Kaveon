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

DB = os.environ.get("METADATA_DATABASE", "kaveonmeta")

VIEW = "kaveon_product_analytics"

# (table, column, data_type, is_dimension, is_metric)
COLUMNS = [
    (VIEW, "usage_date", "date", False, False),
    (VIEW, "user_id", "integer", False, False),
    (VIEW, "queries_run", "integer", False, True),
    (VIEW, "nl_queries", "integer", False, True),
    (VIEW, "sql_lab_runs", "integer", False, True),
    (VIEW, "dashboards_viewed", "integer", False, True),
    (VIEW, "charts_created", "integer", False, True),
    (VIEW, "datasets_accessed", "integer", False, True),
    (VIEW, "exports", "integer", False, True),
    (VIEW, "active_minutes", "numeric", False, True),
    (VIEW, "sessions", "integer", False, True),
    (VIEW, "api_calls", "integer", False, True),
    (VIEW, "data_processed_mb", "numeric", False, True),
    (VIEW, "errors", "integer", False, True),
    (VIEW, "feedback_positive", "integer", False, True),
    (VIEW, "feedback_negative", "integer", False, True),
    (VIEW, "license", "varchar", True, False),
    (VIEW, "audience", "varchar", True, False),
    (VIEW, "segment", "varchar", True, False),
    (VIEW, "industry", "varchar", True, False),
    (VIEW, "team_size", "varchar", True, False),
    (VIEW, "deployment", "varchar", True, False),
    (VIEW, "acquisition_channel", "varchar", True, False),
    (VIEW, "locale", "varchar", True, False),
    (VIEW, "country", "varchar", True, False),
    (VIEW, "region", "varchar", True, False),
    (VIEW, "platform_key", "varchar", True, False),
    (VIEW, "platform", "varchar", True, False),
    (VIEW, "os", "varchar", True, False),
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
    ("Sessions", "SUM(sessions)", "sum", "#,##0"),
    ("API Calls", "SUM(api_calls)", "sum", "#,##0"),
    ("Data Processed MB", "SUM(data_processed_mb)", "sum", "#,##0.0"),
    ("Errors", "SUM(errors)", "sum", "#,##0"),
    ("Positive Feedback", "SUM(feedback_positive)", "sum", "#,##0"),
    ("Negative Feedback", "SUM(feedback_negative)", "sum", "#,##0"),
]


def q(s):
    return s.replace("'", "''")


def main():
    pool.execute_query("SELECT 1", DB)
    # remove prior registration (cascades to dims/cols/metrics via FK ON DELETE CASCADE)
    pool.execute_query("DELETE FROM datasets WHERE dataset_name = 'Kaveon Product Usage'", DB)

    desc = ("Daily per-user product usage of the Kaveon platform — queries run, "
            "NL vs SQL Lab, dashboards viewed, charts created, exports, sessions, "
            "API calls, and active minutes, by license, segment, platform, country "
            "and more. Synthetic.")
    ins = pool.execute_query(
        "INSERT INTO datasets (dataset_name, description, fact_table, schema_name, "
        "database_name, date_column, visibility, created_by) VALUES "
        "('Kaveon Product Usage', '%s', '%s', 'public', 'kaveon', "
        "'usage_date', 'published', 'system') RETURNING id" % (q(desc), VIEW), DB)
    did = ins["rows"][0][0]
    sys.stderr.write("dataset id=%s\n" % did)

    # The VIEW kaveon_product_analytics joins fact + dims into a flat/wide table.
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
