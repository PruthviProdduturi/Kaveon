"""Align the Neon `saved_queries` table with services/saved_queries.py (demo).

The shipped schema_postgresql.sql `saved_queries` was a bare shape; the service
expects the richer MSSQL-shaped columns. Safe: table is empty.
"""
import os
import psycopg2

DDL = """
DROP TABLE IF EXISTS saved_queries CASCADE;
CREATE TABLE saved_queries (
    id                   SERIAL PRIMARY KEY,
    name                 VARCHAR(255) NOT NULL,
    description          TEXT,
    sql_text             TEXT,
    dataset_id           INTEGER,
    tables_used          TEXT,
    run_context          TEXT,
    parameters           TEXT,
    row_limit            INTEGER,
    last_run_at          TIMESTAMP,
    last_run_status      VARCHAR(50),
    last_run_row_count   INTEGER,
    last_run_duration_ms INTEGER,
    created_by           VARCHAR(255),
    created_at           TIMESTAMP NOT NULL DEFAULT NOW(),
    modified_by          VARCHAR(255),
    modified_at          TIMESTAMP NOT NULL DEFAULT NOW(),
    is_shared            BOOLEAN NOT NULL DEFAULT FALSE,
    tags                 TEXT,
    chart_id             INTEGER,
    dashboard_id         VARCHAR(36),
    favorite             BOOLEAN NOT NULL DEFAULT FALSE
);
CREATE INDEX idx_saved_queries_created_by ON saved_queries(created_by);
"""


def main():
    conn = psycopg2.connect(os.environ["NEON_URL"], connect_timeout=30)
    conn.autocommit = True
    conn.cursor().execute(DDL)
    print("saved_queries realigned")
    conn.close()


if __name__ == "__main__":
    main()
