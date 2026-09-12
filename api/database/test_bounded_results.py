"""A query's result set is bounded before it reaches API memory: PostgreSQL and
MySQL statements are wrapped in a LIMIT (their client libraries buffer the
whole result on execute), every driver stops fetching at the bound, and the
caller learns the result was truncated."""
import sys
import unittest
from types import SimpleNamespace
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

import database.pool as pool


class FakeCursor:
    def __init__(self, total):
        self.total = total
        self.description = [("id",), ("v",)]
        self.pos = 0
        self.rowcount = -1
        self.fetchall_called = False

    def execute(self, sql, params=None):
        self.sql = sql

    def fetchmany(self, n):
        out = [[i, f"v{i}"] for i in range(self.pos, min(self.pos + n, self.total))]
        self.pos += len(out)
        return out

    def fetchall(self):
        self.fetchall_called = True
        return self.fetchmany(self.total)

    def close(self):
        pass


class FakeConn:
    def __init__(self, cursor):
        self._cursor = cursor
        self.committed = 0

    def cursor(self):
        return self._cursor

    def commit(self):
        self.committed += 1


class BoundedFetchTests(unittest.TestCase):
    def test_postgres_select_is_wrapped_in_a_limit_and_flagged_truncated(self):
        cur = FakeCursor(total=10_000)
        conn = pool.PostgreSQLConnection.__new__(pool.PostgreSQLConnection)
        conn.connection = FakeConn(cur)
        conn.connect = lambda: None
        fake_pool = SimpleNamespace(db_type="postgresql", get_connection=lambda: conn,
                                    return_connection=lambda c: None, discard_connection=lambda c: None)
        with patch.object(pool, "get_connection_pool", return_value=fake_pool):
            result = pool.execute_query("SELECT id, v FROM big ORDER BY id;", "wh", max_rows=100)
        self.assertEqual(cur.sql, "SELECT * FROM (SELECT id, v FROM big ORDER BY id) AS _bounded LIMIT 101")
        self.assertEqual(len(result["rows"]), 100)
        self.assertTrue(result["truncated"])
        self.assertFalse(cur.fetchall_called)

    def test_a_result_under_the_bound_is_not_truncated(self):
        cur = FakeCursor(total=7)
        conn = pool.PostgreSQLConnection.__new__(pool.PostgreSQLConnection)
        conn.connection = FakeConn(cur)
        conn.connect = lambda: None
        fake_pool = SimpleNamespace(db_type="postgresql", get_connection=lambda: conn,
                                    return_connection=lambda c: None, discard_connection=lambda c: None)
        with patch.object(pool, "get_connection_pool", return_value=fake_pool):
            result = pool.execute_query("SELECT id, v FROM small", "wh", max_rows=100)
        self.assertEqual(result["row_count"], 7)
        self.assertFalse(result["truncated"])

    def test_sql_server_bounds_by_fetching_no_further(self):
        cur = FakeCursor(total=10_000)
        conn = pool.FabricSQLConnection.__new__(pool.FabricSQLConnection)
        conn.connection = FakeConn(cur)
        conn.connect = lambda: None
        fake_pool = SimpleNamespace(db_type="azure_sql", get_connection=lambda: conn,
                                    return_connection=lambda c: None, discard_connection=lambda c: None)
        with patch.object(pool, "get_connection_pool", return_value=fake_pool):
            result = pool.execute_query("SELECT id, v FROM big", "wh", max_rows=50)
        self.assertEqual(cur.sql, "SELECT id, v FROM big")     # no dialect wrap for T-SQL
        self.assertEqual(len(result["rows"]), 50)
        self.assertTrue(result["truncated"])
        self.assertEqual(cur.pos, 51)                          # one row past the bound, then stop

    def test_unbounded_callers_keep_the_full_result(self):
        cur = FakeCursor(total=300)
        conn = pool.PostgreSQLConnection.__new__(pool.PostgreSQLConnection)
        conn.connection = FakeConn(cur)
        conn.connect = lambda: None
        fake_pool = SimpleNamespace(db_type="postgresql", get_connection=lambda: conn,
                                    return_connection=lambda c: None, discard_connection=lambda c: None)
        with patch.object(pool, "get_connection_pool", return_value=fake_pool):
            result = pool.execute_query("SELECT id, v FROM t", "wh")
        self.assertEqual(result["row_count"], 300)
        self.assertFalse(result.get("truncated"))

    def test_non_select_statements_are_never_wrapped(self):
        self.assertEqual(pool._bound_rows("ANALYZE t", "postgresql", 10), "ANALYZE t")
        self.assertEqual(pool._bound_rows("WITH a AS (SELECT 1) SELECT * FROM a", "mysql", 10),
                         "SELECT * FROM (WITH a AS (SELECT 1) SELECT * FROM a) AS _bounded LIMIT 11")


if __name__ == "__main__":
    unittest.main()
