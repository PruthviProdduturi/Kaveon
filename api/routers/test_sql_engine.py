import unittest
from unittest.mock import patch
import sys
from types import SimpleNamespace

# The routing tests do not open a relational connection.  Keep them runnable in
# the lightweight API test environment where the optional ODBC driver is absent.
if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from fastapi import HTTPException, Response

from middleware.auth import UserContext
from models.sql import SqlExecuteBody
from routers import sql
from services.query_generator import build_chart_preview_query


class EngineChartSqlTests(unittest.TestCase):
    def test_active_catalog_is_resolved_server_side_and_result_is_chart_shape(self):
        ctx = UserContext("analyst@example.com", "Analyst")
        body = SqlExecuteBody(
            sql_text="SELECT city, count(*) AS trips FROM nyc_taxi.green_trips GROUP BY city",
            database="OpenSource", dataset_id=7, source="dashboard-chart", row_limit=10,
        )
        source = {"id": "5e4a1605-d533-4c66-86ab-af71c150419c", "engine_catalog": "OpenSource"}
        with patch.object(sql.meta_db, "query_one", return_value=source) as query, \
             patch.object(sql.datasets_svc, "get_dataset_by_id", return_value={"database_name": "OpenSource", "schema_name": "nyc_taxi"}) as dataset, \
             patch.object(sql, "_execute_engine_read_only", return_value={
                 "id": "query-1", "elapsed_ms": 12,
                 "columns": [{"name": "city"}, {"name": "trips"}],
                 "data": [{"city": "Manhattan", "trips": 42}],
             }) as execute, \
             patch.object(sql.sql_execute_limiter, "check"):
            result = sql.execute_engine_sql(body, Response(), ctx)
        self.assertEqual(query.call_args.args[1], ["OpenSource"])
        dataset.assert_called_once_with("7", ctx.email, ctx.role)
        execute.assert_called_once_with(body.sql_text, "OpenSource", ctx, "nyc_taxi")
        self.assertEqual(result, {
            "columns": ["city", "trips"], "rows": [["Manhattan", 42]],
            "query_id": "query-1", "duration_ms": 12,
        })

    def test_missing_or_inactive_catalog_does_not_reach_engine(self):
        body = SqlExecuteBody(sql_text="SELECT 1", database="untrusted", dataset_id=7, source="chart-builder")
        with patch.object(sql.meta_db, "query_one", return_value=None), \
             patch.object(sql.sql_execute_limiter, "check"), \
             patch.object(sql, "_execute_engine_read_only") as execute:
            with self.assertRaises(HTTPException) as error:
                sql.execute_engine_sql(body, Response(), UserContext("analyst@example.com", "Analyst"))
        self.assertEqual(error.exception.status_code, 404)
        execute.assert_not_called()

    def test_engine_query_requires_an_authorized_dataset_schema(self):
        body = SqlExecuteBody(sql_text="SELECT 1", database="OpenSource", source="chart-builder")
        source = {"id": "source", "engine_catalog": "OpenSource"}
        with patch.object(sql.meta_db, "query_one", return_value=source), \
             patch.object(sql.sql_execute_limiter, "check"), \
             patch.object(sql, "_execute_engine_read_only") as execute:
            with self.assertRaises(HTTPException) as error:
                sql.execute_engine_sql(body, Response(), UserContext("analyst@example.com", "Analyst"))
        self.assertEqual(error.exception.status_code, 400)
        execute.assert_not_called()

    def test_viewer_cannot_execute_engine_sql_even_with_dashboard_source(self):
        body = SqlExecuteBody(sql_text="SELECT 1", database="OpenSource", source="dashboard-chart")
        with patch.object(sql.meta_db, "query_one") as lookup, \
             patch.object(sql.sql_execute_limiter, "check"):
            with self.assertRaises(HTTPException) as error:
                sql.execute_engine_sql(body, Response(), UserContext("viewer@example.com", "Viewer"))
        self.assertEqual(error.exception.status_code, 403)
        lookup.assert_not_called()

    def test_virtual_dataset_sql_is_generated_without_client_sql(self):
        dataset = {
            "id": "7", "schema_name": None, "table_name": None,
            "database_name": "OpenSource", "sql_text": "SELECT model, score FROM ai_benchmarks.leaderboard",
            "dimensions": [], "columns": [],
        }
        body = sql.SqlGenerateBody(
            dataset_id=7,
            chart_type="bar",
            config={"groupby": ["model"], "datasource": "attacker.table", "sql_text": "SELECT 0"},
        )
        with patch.object(sql.datasets_svc, "get_dataset_by_id", return_value=dataset), \
             patch.object(sql, "_is_engine_catalog", return_value=True), \
             patch.object(sql, "build_chart_preview_query", return_value="SELECT \"model\" FROM (SELECT model FROM ai_benchmarks.leaderboard)") as generate:
            result = sql.generate_sql(body, UserContext("analyst@example.com", "Analyst"))
        self.assertEqual(result["sql_text"], "SELECT \"model\" FROM (SELECT model FROM ai_benchmarks.leaderboard)")
        params = generate.call_args.args[0]
        self.assertEqual(params["datasource"], "")
        self.assertEqual(params["sql_text"], dataset["sql_text"])
        self.assertEqual(params["database_name"], "OpenSource")
        self.assertEqual(params["db_type"], "postgresql")

    def test_virtual_engine_dataset_uses_portable_quotes_not_tsql_top(self):
        generated = build_chart_preview_query({
            "datasource": "",
            "sql_text": "SELECT model, score FROM ai_benchmarks.leaderboard",
            "db_type": "postgresql",
            "engine_virtual_source": True,
            "groupby": ["model"],
            "metrics": [{"column": "score", "aggregate": "AVG", "label": "average_score"}],
        })
        self.assertIsNotNone(generated)
        self.assertIn('FROM (\nSELECT model, score FROM ai_benchmarks.leaderboard\n)', generated)
        self.assertIn('"model"', generated)
        self.assertNotIn("TOP ", generated)
        self.assertNotIn("[model]", generated)

    def test_postgresql_aggregate_cap_uses_final_limit(self):
        generated = build_chart_preview_query({
            "datasource": "",
            "sql_text": (
                "SELECT pickup_date, SUM(trip_count) AS trips "
                "FROM OpenSource.nyc_taxi.daily_trips GROUP BY pickup_date ORDER BY pickup_date"
            ),
            "db_type": "postgresql",
            "engine_virtual_source": True,
            "groupby": ["pickup_date"],
            "metrics": [{"column": "trips", "aggregate": "MAX", "label": "trips"}],
            "row_limit": 500,
            "sort_by": {"column": "pickup_date", "direction": "asc"},
        })
        self.assertIsNotNone(generated)
        self.assertNotIn("TOP ", generated)
        self.assertTrue(generated.endswith("LIMIT 500"), generated)
        self.assertNotIn("fact.", generated)
        self.assertIn('ORDER BY "pickup_date" ASC', generated)

    def test_engine_virtual_metric_sort_uses_projected_alias(self):
        generated = build_chart_preview_query({
            "datasource": "", "sql_text": "SELECT service_type, 5 AS trips FROM services",
            "db_type": "postgresql", "engine_virtual_source": True,
            "groupby": ["service_type"],
            "metrics": [{"column": "trips", "aggregate": "MAX", "label": "trips"}],
            "sort_by": {"column": "trips", "direction": "desc"},
        })
        self.assertIn('ORDER BY "trips" DESC', generated)
        self.assertNotIn('ORDER BY MAX(', generated)

    def test_non_engine_postgresql_virtual_query_keeps_its_relation_alias(self):
        generated = build_chart_preview_query({
            "datasource": "", "sql_text": "SELECT model, score FROM leaderboard",
            "db_type": "postgresql", "groupby": ["model"],
            "metrics": [{"column": "score", "aggregate": "AVG"}],
        })
        self.assertIn('fact."model"', generated)

    def test_engine_virtual_source_text_is_not_rewritten(self):
        source_sql = "SELECT 'fact.score' AS note, score FROM leaderboard"
        generated = build_chart_preview_query({
            "datasource": "", "sql_text": source_sql, "db_type": "postgresql",
            "engine_virtual_source": True, "groupby": ["note"],
            "metrics": [{"column": "score", "aggregate": "MAX"}],
        })
        self.assertIn("'fact.score'", generated)
        self.assertNotIn('fact."note"', generated)


if __name__ == "__main__":
    unittest.main()
