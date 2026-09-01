"""Apply the Kaveon Postgres schema to the target DB ($NEON_URL)."""
import os
import psycopg2

URL = os.environ["NEON_URL"]
SCHEMA = os.path.join(os.path.dirname(__file__), "..", "api", "schema_postgresql.sql")


def main():
    sql = open(SCHEMA, encoding="utf-8").read()
    conn = psycopg2.connect(URL, connect_timeout=30)
    conn.autocommit = True
    cur = conn.cursor()
    cur.execute(sql)
    cur.execute(
        "select table_name from information_schema.tables "
        "where table_schema='public' order by table_name"
    )
    print("Tables:", ", ".join(r[0] for r in cur.fetchall()))
    cur.close()
    conn.close()


if __name__ == "__main__":
    main()
