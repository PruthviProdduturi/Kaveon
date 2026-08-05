"""Align the Neon `charts` table with the service layer (demo-scoped).

Kaveon is MSSQL/Fabric-first; the shipped schema_postgresql.sql `charts` table was
out of sync with services/charts.py (which uses query_config/viz_config + auto id).
This recreates it to match so charts persist on the Neon demo DB. Safe: no charts
exist yet.
"""
import os
import psycopg2

DDL = """
DROP TABLE IF EXISTS charts CASCADE;
CREATE TABLE charts (
    id           SERIAL       PRIMARY KEY,
    name         VARCHAR(255) NOT NULL,
    description  TEXT         NULL,
    chart_type   VARCHAR(50)  NOT NULL,
    query_config TEXT         NOT NULL DEFAULT '{}',
    viz_config   TEXT         NOT NULL DEFAULT '{}',
    visibility   VARCHAR(20)  NOT NULL DEFAULT 'internal'
                 CONSTRAINT ck_charts_visibility CHECK (visibility IN ('private','internal','published')),
    created_on   TIMESTAMP    NULL,
    changed_on   TIMESTAMP    NULL,
    created_at   TIMESTAMP    NOT NULL DEFAULT NOW(),
    updated_at   TIMESTAMP    NOT NULL DEFAULT NOW(),
    created_by   VARCHAR(255) NOT NULL DEFAULT 'system',
    updated_by   VARCHAR(255) NULL
);
CREATE INDEX idx_charts_name       ON charts(name);
CREATE INDEX idx_charts_visibility ON charts(visibility);
CREATE INDEX idx_charts_updated_at ON charts(updated_at);
"""


def main():
    conn = psycopg2.connect(os.environ["NEON_URL"], connect_timeout=30)
    conn.autocommit = True
    cur = conn.cursor()
    cur.execute(DDL)
    cur.execute("select column_name from information_schema.columns where table_name='charts' order by ordinal_position")
    print("charts columns:", ", ".join(r[0] for r in cur.fetchall()))
    cur.close()
    conn.close()


if __name__ == "__main__":
    main()
