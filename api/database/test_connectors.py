import unittest
from unittest.mock import patch

from database.connectors import execute_registered_source


class RegisteredSourceExecutorTests(unittest.TestCase):
    def test_pushes_one_bounded_statement_through_the_pool(self):
        expected = {"columns": ["id"], "rows": [[1]], "row_count": 1}
        with patch(
            "database.connectors.pool.execute_query", return_value=expected
        ) as execute:
            result = execute_registered_source(
                "SELECT id FROM orders",
                "warehouse",
                timeout_seconds=12,
                max_rows=100,
                actor="analyst@example.com",
                role="Analyst",
            )

        self.assertIs(result, expected)
        execute.assert_called_once_with(
            "SELECT id FROM orders",
            "warehouse",
            params=None,
            timeout_seconds=12,
            max_rows=100,
            actor="analyst@example.com",
            role="Analyst",
        )

    def test_registered_source_execution_never_fans_out_to_multiple_sources(self):
        """A registered-source statement has exactly one source authority.

        The connector seam deliberately pushes the complete statement to the
        selected source.  This regression keeps cross-source joins from being
        implied by a future refactor until a federated planner, snapshot
        contract, and failure semantics are qualified separately.
        """
        with patch("database.connectors.pool.execute_query", return_value={
            "columns": ["id"], "rows": [[1]], "row_count": 1,
        }) as execute:
            execute_registered_source(
                "SELECT a.id FROM orders a JOIN customers c ON c.id = a.customer_id",
                "warehouse",
                actor="analyst@example.com",
                role="Analyst",
            )

        self.assertEqual(execute.call_count, 1)
        self.assertEqual(execute.call_args.args[1], "warehouse")
        self.assertEqual(execute.call_args.args[0].count(" JOIN "), 1)

    def test_source_failure_propagates_without_retry_or_fanout(self):
        failure = TimeoutError("warehouse statement timed out")
        with patch(
            "database.connectors.pool.execute_query", side_effect=failure
        ) as execute:
            with self.assertRaises(TimeoutError) as raised:
                execute_registered_source(
                    "SELECT id FROM orders",
                    "warehouse",
                    timeout_seconds=5,
                    actor="analyst@example.com",
                    role="Analyst",
                )

        self.assertIs(raised.exception, failure)
        execute.assert_called_once_with(
            "SELECT id FROM orders",
            "warehouse",
            params=None,
            timeout_seconds=5,
            max_rows=None,
            actor="analyst@example.com",
            role="Analyst",
        )


if __name__ == "__main__":
    unittest.main()
