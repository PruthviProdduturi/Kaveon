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
from services.query_generator import build_chart_preview_query, build_distinct_filter_values_query


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

    def test_failed_engine_uuid_and_details_are_persisted_in_query_history(self):
        body = SqlExecuteBody(sql_text="SELECT broken FROM events", database="OpenSource",
                              dataset_id=7, source="dashboard-chart")
        failure = HTTPException(422, {
            "message": "Engine query failed", "query_id": "engine-failed-1",
            "engine_details": {"id": "engine-failed-1", "state": "FAILED"},
        })
        with patch.object(sql, "_engine_source_for_catalog", return_value={"engine_catalog": "OpenSource"}), \
             patch.object(sql.datasets_svc, "get_dataset_by_id", return_value={"database_name": "OpenSource", "schema_name": "app"}), \
             patch.object(sql, "_execute_engine_read_only", side_effect=failure), \
             patch.object(sql.history_svc, "create_history") as history, \
             patch.object(sql.sql_execute_limiter, "check"):
            with self.assertRaises(HTTPException) as raised:
                sql.execute_engine_sql(body, Response(), UserContext("analyst@example.com", "Analyst"))
        self.assertEqual(raised.exception.detail, "Engine query failed")
        recorded = history.call_args.args[0]
        self.assertEqual(recorded["status"], "error")
        self.assertEqual(recorded["engine_query_id"], "engine-failed-1")
        self.assertEqual(recorded["engine_details"]["state"], "FAILED")

    def test_viewer_cannot_execute_engine_sql_even_with_dashboard_source(self):
        body = SqlExecuteBody(sql_text="SELECT 1", database="OpenSource", source="dashboard-chart")
        with patch.object(sql.meta_db, "query_one") as lookup, \
             patch.object(sql.sql_execute_limiter, "check"):
            with self.assertRaises(HTTPException) as error:
                sql.execute_engine_sql(body, Response(), UserContext("viewer@example.com", "Viewer"))
        self.assertEqual(error.exception.status_code, 403)
        lookup.assert_not_called()

    def test_engine_admission_retry_is_not_rewritten_as_a_server_error(self):
        dataset = {"id": "7", "database_name": "OpenSource", "schema_name": "nyc_taxi",
                   "table_name": "daily_trips", "dimensions": [], "columns": []}
        retryable = HTTPException(429, "Engine query capacity is temporarily exhausted", headers={"Retry-After": "1"})
        with patch.object(sql.datasets_svc, "get_dataset_by_id", return_value=dataset), \
             patch.object(sql, "_engine_source_for_catalog", return_value={"engine_catalog": "OpenSource"}), \
             patch.object(sql, "build_distinct_filter_values_query", return_value={"sql": "SELECT 1", "keyColumn": "country", "filteringTier": "fact"}), \
             patch.object(sql, "_execute_engine_read_only", side_effect=retryable):
            with self.assertRaises(HTTPException) as error:
                sql.distinct_filter_values(Response(), "7", "country", fact_key=None, limit=100,
                                            source=None, chart_id=None, dashboard_id=None, filters=None,
                                            ctx=UserContext("analyst@example.com", "Analyst"))
        self.assertEqual(error.exception.status_code, 429)
        self.assertEqual(error.exception.headers, {"Retry-After": "1"})

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
        self.assertEqual(params["db_type"], "kaveon")

    def test_kaveon_time_grains_use_prebucketed_engine_column(self):
        for grain in ("week", "month", "quarter", "year"):
            generated = build_chart_preview_query({
                "datasource": "climate.temperature", "db_type": "kaveon", "engine_source": True,
                "time_column": "observed_at", "time_grain": grain,
                "metrics": [{"column": "value", "aggregate": "AVG", "label": "Average"}],
            })
            self.assertIn("observed_at AS date", generated)
            self.assertNotIn("::date", generated)

    def test_kaveon_date_display_formats_have_no_postgres_cast_operator(self):
        month = build_chart_preview_query({
            "datasource": "climate.temperature", "db_type": "kaveon", "engine_source": True,
            "time_column": "observed_at", "date_display_format": "month",
            "metrics": [{"column": "value", "aggregate": "SUM"}],
        })
        quarter_year = build_chart_preview_query({
            "datasource": "climate.temperature", "db_type": "kaveon", "engine_source": True,
            "time_column": "observed_at", "date_display_format": "quarter-year",
            "metrics": [{"column": "value", "aggregate": "SUM"}],
        })
        self.assertIn("observed_at AS date", month)
        self.assertIn("observed_at AS date", quarter_year)
        self.assertNotIn("::", quarter_year)

    def test_kaveon_relative_date_ranges_remain_datafusion_compatible(self):
        generated = build_chart_preview_query({
            "datasource": "climate.temperature", "db_type": "kaveon", "engine_source": True,
            "time_column": "observed_at", "time_grain": "month", "time_range": "previous_month",
            "metrics": [{"column": "value", "aggregate": "SUM"}],
        })
        self.assertIn("DATE_TRUNC('month', CURRENT_DATE) - INTERVAL '1 month'", generated)
        self.assertNotIn("GETUTCDATE", generated)
        self.assertNotIn("::", generated)

    def test_virtual_engine_dataset_uses_portable_quotes_not_tsql_top(self):
        generated = build_chart_preview_query({
            "datasource": "",
            "sql_text": "SELECT model, score FROM ai_benchmarks.leaderboard",
            "db_type": "postgresql",
            "engine_virtual_source": True,
            "engine_source": True,
            "groupby": ["model"],
            "metrics": [{"column": "score", "aggregate": "AVG", "label": "average_score"}],
        })
        self.assertIsNotNone(generated)
        self.assertIn('FROM (\nSELECT model, score FROM ai_benchmarks.leaderboard\n)', generated)
        self.assertIn("SELECT model", generated)
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
            "engine_source": True,
            "groupby": ["pickup_date"],
            "metrics": [{"column": "trips", "aggregate": "MAX", "label": "trips"}],
            "row_limit": 500,
            "sort_by": {"column": "pickup_date", "direction": "asc"},
        })
        self.assertIsNotNone(generated)
        self.assertNotIn("TOP ", generated)
        self.assertTrue(generated.endswith("LIMIT 500"), generated)
        self.assertNotIn("fact.", generated)
        self.assertIn("ORDER BY pickup_date ASC", generated)

    def test_engine_virtual_metric_sort_uses_projected_alias(self):
        generated = build_chart_preview_query({
            "datasource": "", "sql_text": "SELECT service_type, 5 AS trips FROM services",
            "db_type": "postgresql", "engine_virtual_source": True, "engine_source": True,
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
            "engine_virtual_source": True, "engine_source": True, "groupby": ["note"],
            "metrics": [{"column": "score", "aggregate": "MAX"}],
        })
        self.assertIn("'fact.score'", generated)
        self.assertNotIn('fact."note"', generated)

    def test_physical_engine_metric_sort_uses_projected_alias(self):
        generated = build_chart_preview_query({
            "datasource": "nyc_taxi.daily_trips", "db_type": "postgresql", "engine_source": True,
            "groupby": ["service_type"],
            "metrics": [{"column": "trip_count", "aggregate": "SUM", "label": "trips"}],
            "sort_by": {"column": "trips", "direction": "desc"}, "row_limit": 20,
        })
        self.assertIn('ORDER BY "trips" DESC', generated)
        self.assertNotIn('ORDER BY SUM(', generated)
        self.assertTrue(generated.endswith("LIMIT 20"), generated)

    def test_engine_metric_sort_matches_hydrated_qualified_column(self):
        generated = build_chart_preview_query({
            "datasource": "kaveon_product.kaveon_product_analytics",
            "db_type": "postgresql", "engine_source": True,
            "groupby": ["platform"],
            "metrics": [{"column": "kaveon_product_analytics.queries_run", "aggregate": "SUM", "label": "Queries"}],
            "sort_by": {"column": "queries_run", "direction": "desc"}, "row_limit": 500,
        })
        self.assertIn('ORDER BY "Queries" DESC', generated)
        self.assertNotIn('ORDER BY SUM(queries_run)', generated)

    def test_physical_engine_presentation_metric_aliases_are_quoted(self):
        generated = build_chart_preview_query({
            "datasource": "kaveon_product.kaveon_product_analytics",
            "db_type": "postgresql", "engine_source": True,
            "groupby": ["segment"],
            "metrics": [
                {"column": "user_id", "aggregate": "COUNT_DISTINCT", "label": "Active Users"},
                {"column": "active_minutes", "aggregate": "AVG", "label": "Avg Min"},
                {"column": "nl_queries", "aggregate": "SUM", "label": "NL Queries"},
                {"column": "data_processed_mb", "aggregate": "SUM", "label": "MB Processed"},
            ],
            "sort_by": {"column": "user_id", "direction": "desc"},
        })
        self.assertIn('COUNT(DISTINCT user_id) AS "Active Users"', generated)
        self.assertIn('AVG(active_minutes) AS "Avg Min"', generated)
        self.assertIn('SUM(nl_queries) AS "NL Queries"', generated)
        self.assertIn('SUM(data_processed_mb) AS "MB Processed"', generated)
        self.assertIn('ORDER BY "Active Users" DESC', generated)

    def test_physical_engine_count_distinct_and_date_filters_are_sql_literals(self):
        generated = build_chart_preview_query({
            "datasource": "nyc_taxi.daily_trips", "db_type": "postgresql", "engine_source": True,
            "groupby": ["pickup_date"],
            "metrics": [{"column": "service_type", "aggregate": "COUNT_DISTINCT", "label": "services"}],
            "filters": [
                {"column": "pickup_date", "operator": ">=", "value": "2025-01-01"},
                {"column": "pickup_date", "operator": "<=", "value": "2025-01-31"},
            ],
        })
        self.assertIn("COUNT(DISTINCT service_type)", generated)
        self.assertIn("pickup_date >= '2025-01-01'", generated)
        self.assertIn("pickup_date <= '2025-01-31'", generated)
        self.assertNotIn("fact.", generated)

    def test_physical_engine_distinct_filter_query_uses_limit_and_quotes(self):
        generated = build_distinct_filter_values_query({
            "datasource": "nyc_taxi.daily_trips", "column": "service_type",
            "db_type": "postgresql", "engine_source": True, "limit": 50, "dimensions": [], "columns": [],
        })
        self.assertIsNotNone(generated)
        self.assertIn("service_type", generated["sql"])
        self.assertTrue(generated["sql"].endswith("LIMIT 50"), generated["sql"])


if __name__ == "__main__":
    unittest.main()
