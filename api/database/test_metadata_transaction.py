import unittest
from unittest.mock import patch
import sys
from types import SimpleNamespace

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from database import metadata


class FakeCursor:
    description = None
    rowcount = 1

    def __init__(self, failure=None):
        self.failure = failure
        self.closed = False

    def execute(self, sql, params=None):
        if self.failure:
            raise self.failure

    def close(self):
        self.closed = True


class FakeRawConnection:
    def __init__(self, cursor=None):
        self.autocommit = True
        self._cursor = cursor or FakeCursor()
        self.commits = 0
        self.rollbacks = 0

    def cursor(self):
        return self._cursor

    def commit(self):
        self.commits += 1

    def rollback(self):
        self.rollbacks += 1


class FakeConnection:
    def __init__(self, raw):
        self.connection = raw

    def connect(self):
        pass


class FakePool:
    db_type = "postgresql"

    def __init__(self, connection):
        self.connection = connection
        self.returned = 0
        self.discarded = 0

    def get_connection(self):
        return self.connection

    def return_connection(self, connection):
        self.returned += 1

    def discard_connection(self, connection):
        self.discarded += 1


class MetadataTransactionTests(unittest.TestCase):
    def test_success_commits_once_and_restores_pooled_connection(self):
        raw = FakeRawConnection()
        pool = FakePool(FakeConnection(raw))
        with patch.dict("os.environ", {"METADATA_DATABASE": "metadata"}), \
             patch.object(metadata, "get_connection_pool", return_value=pool):
            with metadata.transaction() as tx:
                self.assertEqual(tx.execute("UPDATE products SET name = @param0", ["new"]), 1)
                self.assertFalse(raw.autocommit)
        self.assertEqual((raw.commits, raw.rollbacks, pool.returned), (1, 0, 1))
        self.assertTrue(raw.autocommit)

    def test_failure_rolls_back_every_statement_and_returns_healthy_connection(self):
        failure = RuntimeError("injected child insert failure")
        raw = FakeRawConnection(FakeCursor(failure))
        pool = FakePool(FakeConnection(raw))
        with patch.dict("os.environ", {"METADATA_DATABASE": "metadata"}), \
             patch.object(metadata, "get_connection_pool", return_value=pool), \
             patch.object(metadata, "is_connection_error", return_value=False):
            with self.assertRaisesRegex(RuntimeError, "injected"):
                with metadata.transaction() as tx:
                    tx.execute("INSERT INTO child VALUES (@param0)", [1])
        self.assertEqual((raw.commits, raw.rollbacks, pool.returned), (0, 1, 1))
        self.assertTrue(raw.autocommit)

    def test_non_postgres_metadata_backend_fails_before_checkout(self):
        pool = FakePool(FakeConnection(FakeRawConnection()))
        pool.db_type = "azure_sql"
        with patch.dict("os.environ", {"METADATA_DATABASE": "metadata"}), \
             patch.object(metadata, "get_connection_pool", return_value=pool):
            with self.assertRaisesRegex(RuntimeError, "require PostgreSQL"):
                with metadata.transaction():
                    pass
        self.assertEqual(pool.returned, 0)


if __name__ == "__main__":
    unittest.main()
