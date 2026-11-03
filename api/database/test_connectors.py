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


if __name__ == "__main__":
    unittest.main()
