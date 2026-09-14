import os
import unittest
from unittest.mock import patch

from services import postgresql_write_fence as fence


class PostgreSQLWriteFenceTests(unittest.TestCase):
    def test_default_is_open_and_exact_true_enables_fence(self):
        with patch.dict(os.environ, {}, clear=True):
            self.assertFalse(fence.enabled())
            fence.assert_allowed("DELETE FROM datasets", "postgresql")
        with patch.dict(os.environ, {fence.ENVIRONMENT_KEY: "true"}, clear=True):
            self.assertTrue(fence.enabled())

    def test_blocks_authority_mutations_and_allows_reads(self):
        with patch.dict(os.environ, {fence.ENVIRONMENT_KEY: "true"}, clear=True):
            fence.assert_allowed("SELECT * FROM datasets", "postgresql")
            fence.assert_allowed("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY", "postgresql")
            for statement in (
                "INSERT INTO dbo.datasets (id) VALUES (1)",
                "UPDATE charts SET name = 'new' WHERE id = 1",
                "DELETE FROM query_history WHERE id = 1",
                "TRUNCATE TABLE chat_messages",
                "ALTER TABLE dashboards ADD COLUMN x INT",
            ):
                with self.subTest(statement=statement), self.assertRaises(fence.PostgreSQLWriteFencedError):
                    fence.assert_allowed(statement, "postgresql")

    def test_comments_and_values_cannot_create_false_positive(self):
        with patch.dict(os.environ, {fence.ENVIRONMENT_KEY: "true"}, clear=True):
            fence.assert_allowed("SELECT 'DELETE FROM datasets' AS text -- UPDATE charts", "postgresql")

    def test_other_backends_and_non_authority_tables_are_outside_fence(self):
        with patch.dict(os.environ, {fence.ENVIRONMENT_KEY: "true"}, clear=True):
            fence.assert_allowed("DELETE FROM datasets", "fabric_sql")
            fence.assert_allowed("UPDATE operational_health SET value = 1", "postgresql")


class MetadataBoundaryTests(unittest.TestCase):
    def test_public_query_checks_fence_before_pool_execution(self):
        from database import metadata

        environment = {
            "METADATA_DATABASE": "metadata",
            "METADATA_DB_TYPE": "postgresql",
            fence.ENVIRONMENT_KEY: "true",
        }
        with patch.dict(os.environ, environment, clear=True), \
             patch.object(metadata, "execute_query") as execute_query, \
             self.assertRaises(fence.PostgreSQLWriteFencedError):
            metadata.execute("DELETE FROM datasets WHERE id = @param0", [1])
        execute_query.assert_not_called()

    def test_transaction_query_checks_fence_before_cursor(self):
        from database.metadata import MetadataTransaction

        class Connection:
            connection = unittest.mock.Mock()

        transaction = MetadataTransaction(Connection(), "postgresql")
        with patch.dict(os.environ, {fence.ENVIRONMENT_KEY: "true"}, clear=True), \
             self.assertRaises(fence.PostgreSQLWriteFencedError):
            transaction.execute("UPDATE dashboards SET name = @param0", ["x"])
        Connection.connection.cursor.assert_not_called()


if __name__ == "__main__":
    unittest.main()
