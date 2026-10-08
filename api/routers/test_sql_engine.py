import time
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
        execute.assert_called_once_with(body.sql_text, "OpenSource", ctx, "nyc_taxi",
                                        cancel_token=None)
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


class EngineCancellationTests(unittest.TestCase):
    def test_cancel_token_is_scoped_to_the_caller(self):
        """The token is minted in the browser, so it must not be the Engine tag
        itself: one principal's token has to be unable to name — and so cancel —
        another principal's statement."""
        first = sql.UserContext(email="analyst@example.com", role="Analyst", jwt_roles=[])
        second = sql.UserContext(email="other@example.com", role="Analyst", jwt_roles=[])
        token = "abcd1234efgh"
        self.assertNotEqual(sql._cancel_tag(first, token), sql._cancel_tag(second, token))
        self.assertEqual(sql._cancel_tag(first, token), sql._cancel_tag(first, token))
        self.assertIsNone(sql._cancel_tag(first, None))

    def test_cancel_rejects_a_malformed_token_before_reaching_the_engine(self):
        ctx = sql.UserContext(email="analyst@example.com", role="Analyst", jwt_roles=[])
        with patch("services.engine_bridge.cancel_tagged") as cancel:
            with self.assertRaises(sql.HTTPException):
                sql.cancel_engine_statement("short", ctx)
            with self.assertRaises(sql.HTTPException):
                sql.cancel_engine_statement("has spaces and/slashes", ctx)
            cancel.assert_not_called()

    def test_cancel_reports_nothing_cancelled_rather_than_failing(self):
        """A statement that finished on its own is simply not running; the
        reader navigating away must not see an error for that."""
        ctx = sql.UserContext(email="analyst@example.com", role="Analyst", jwt_roles=[])
        with patch("services.engine_bridge.cancel_tagged", return_value=0) as cancel:
            self.assertEqual(sql.cancel_engine_statement("abcd1234efgh", ctx),
                             {"ok": True, "cancelled": 0})
        self.assertEqual(cancel.call_args.args[0],
                         sql._cancel_tag(ctx, "abcd1234efgh"))


class EngineCubeRewriteRoutingTests(unittest.TestCase):
    """How the chart path uses the cube rewrite, and when it gives it up."""

    CTX = UserContext("analyst@example.com", "Analyst")
    SORTED_SQL = ('SELECT region, SUM(actions) AS "A" FROM public.events '
                  'GROUP BY region ORDER BY "A" DESC NULLS LAST LIMIT 2')
    REWRITTEN = ('SELECT region, SUM(actions) AS "A" FROM public.events '
                 'GROUP BY region')
    AVERAGE_SQL = ('SELECT surface, AVG(latency) AS "Avg Latency (ms)" '
                   'FROM public.events GROUP BY surface '
                   'ORDER BY "Avg Latency (ms)" DESC NULLS LAST LIMIT 2')
    AVERAGE_REWRITTEN = (
        'SELECT surface, SUM(latency) AS "__kaveon_avg_sum_1", '
        'COUNT(latency) AS "__kaveon_avg_count_1" '
        'FROM public.events GROUP BY surface')

    def setUp(self):
        sql._ENGINE_CUBE_DECLINED.clear()

    tearDown = setUp

    @staticmethod
    def result(rows, names=("region", "A")):
        return {"id": "query-1", "elapsed_ms": 150,
                "columns": [{"name": name} for name in names], "data": rows}

    def run_chart(self, side_effect, sql_text=None):
        with patch.object(sql, "_execute_engine_read_only",
                          side_effect=side_effect) as execute:
            result, finished = sql._execute_engine_chart_statement(
                sql_text or self.SORTED_SQL, "OpenSource", self.CTX, "public", None)
        return execute, result, finished

    def test_the_cube_shaped_statement_is_issued_and_finished_in_the_api(self):
        rows = [["Asia", 2], ["Europe", 3], ["Oceania", 1]]
        execute, result, finished = self.run_chart([self.result(rows)])
        self.assertEqual(execute.call_count, 1)
        self.assertEqual(execute.call_args.args[0], self.REWRITTEN)
        self.assertEqual(execute.call_args.kwargs["timeout"], sql.ENGINE_CUBE_PROBE_SECONDS)
        self.assertEqual(result["id"], "query-1")
        self.assertEqual(finished, (["region", "A"], [["Europe", 3], ["Asia", 2]]))

    def test_an_average_is_issued_as_a_sum_and_a_count_and_divided_here(self):
        execute, _, finished = self.run_chart(
            [self.result([["Chat", 9, 3], ["API", 10, 4]],
                         names=("surface", "__kaveon_avg_sum_1", "__kaveon_avg_count_1"))],
            self.AVERAGE_SQL)
        self.assertEqual(execute.call_args.args[0], self.AVERAGE_REWRITTEN)
        # The caller sees its own two columns, under its own names, ordered by
        # the quotient rather than by the sum that produced it.
        self.assertEqual(finished,
                         (["surface", "Avg Latency (ms)"], [["Chat", 3.0], ["API", 2.5]]))

    def test_a_statement_outside_the_recognised_shape_is_run_as_written(self):
        unsupported = "SELECT * FROM public.events ORDER BY region LIMIT 2"
        with patch.object(sql, "_execute_engine_read_only",
                          return_value=self.result([["Asia", 2]])) as execute:
            _, finished = sql._execute_engine_chart_statement(
                unsupported, "OpenSource", self.CTX, "public", None)
        self.assertIsNone(finished)
        self.assertEqual(execute.call_args.args[0], unsupported)
        self.assertNotIn("timeout", execute.call_args.kwargs)

    def test_a_result_wider_than_the_cap_falls_back_to_the_caller_s_statement(self):
        wide = [[str(index), index] for index in range(sql.ENGINE_CUBE_ROW_CAP + 1)]
        execute, _, finished = self.run_chart(
            [self.result(wide), self.result([["Asia", 2]])])
        self.assertIsNone(finished)
        self.assertEqual(execute.call_count, 2)
        self.assertEqual(execute.call_args_list[0].args[0], self.REWRITTEN)
        self.assertEqual(execute.call_args_list[1].args[0], self.SORTED_SQL)

    def test_a_result_the_api_cannot_order_falls_back_to_the_caller_s_statement(self):
        mixed = [["Asia", 2], ["Europe", "three"]]
        execute, _, finished = self.run_chart(
            [self.result(mixed), self.result([["Asia", 2]])])
        self.assertIsNone(finished)
        self.assertEqual(execute.call_count, 2)
        self.assertEqual(execute.call_args_list[1].args[0], self.SORTED_SQL)

    def test_a_result_the_api_cannot_divide_falls_back_to_the_caller_s_statement(self):
        unusable = self.result(
            [["Chat", 9, "three"]],
            names=("surface", "__kaveon_avg_sum_1", "__kaveon_avg_count_1"))
        execute, _, finished = self.run_chart(
            [unusable, self.result([["Chat", 3.0]])], self.AVERAGE_SQL)
        self.assertIsNone(finished)
        self.assertEqual(execute.call_count, 2)
        self.assertEqual(execute.call_args_list[1].args[0], self.AVERAGE_SQL)

    def test_a_rewrite_that_did_not_complete_falls_back_and_is_not_retried(self):
        timeout = HTTPException(504, "Engine statement exceeded the client bound (5s)")
        execute, _, finished = self.run_chart([timeout, self.result([["Asia", 2]])])
        self.assertIsNone(finished)
        self.assertEqual(execute.call_count, 2)
        # The outcome is remembered, so the next execution goes straight to
        # the caller's own statement rather than probing again.
        with patch.object(sql, "_execute_engine_read_only",
                          return_value=self.result([["Asia", 2]])) as again:
            sql._execute_engine_chart_statement(
                self.SORTED_SQL, "OpenSource", self.CTX, "public", None)
        self.assertEqual(again.call_count, 1)
        self.assertEqual(again.call_args.args[0], self.SORTED_SQL)

    def test_a_refusal_or_exhausted_admission_queue_is_reported_not_retried(self):
        for status in (403, 429):
            sql._ENGINE_CUBE_DECLINED.clear()
            refusal = HTTPException(status, "refused")
            with patch.object(sql, "_execute_engine_read_only",
                              side_effect=refusal) as execute:
                with self.assertRaises(HTTPException) as error:
                    sql._execute_engine_chart_statement(
                        self.SORTED_SQL, "OpenSource", self.CTX, "public", None)
            self.assertEqual(error.exception.status_code, status)
            self.assertEqual(execute.call_count, 1)

    def test_a_declined_rewrite_expires(self):
        key = sql._cache_key("OpenSource", self.REWRITTEN)
        sql._decline_engine_cube(key)
        self.assertTrue(sql._engine_cube_declined(key))
        sql._ENGINE_CUBE_DECLINED[key] = time.time() - sql._ENGINE_CUBE_DECLINED_TTL - 1
        self.assertFalse(sql._engine_cube_declined(key))

    def route(self, sql_text, result):
        body = SqlExecuteBody(sql_text=sql_text, database="OpenSource",
                              dataset_id=7, source="dashboard-chart")
        dataset = {"database_name": "OpenSource", "schema_name": "public"}
        with patch.object(sql, "_engine_source_for_catalog",
                          return_value={"engine_catalog": "OpenSource"}), \
             patch.object(sql.datasets_svc, "get_dataset_by_id", return_value=dataset), \
             patch.object(sql, "_execute_engine_read_only", return_value=result), \
             patch.object(sql.history_svc, "create_history"), \
             patch.object(sql.sql_execute_limiter, "check"):
            return sql.execute_engine_sql(body, Response(), self.CTX)

    def test_the_route_returns_the_finished_rows_in_the_unchanged_response_shape(self):
        answer = self.route(
            self.SORTED_SQL,
            self.result([["Asia", 2], ["Europe", 3], ["Oceania", 1]]))
        self.assertEqual(answer, {
            "columns": ["region", "A"], "rows": [["Europe", 3], ["Asia", 2]],
            "query_id": "query-1", "duration_ms": 150,
        })

    def test_the_route_never_leaks_the_substituted_sum_and_count(self):
        answer = self.route(
            self.AVERAGE_SQL,
            self.result([["Chat", 9, 3], ["API", 10, 4]],
                        names=("surface", "__kaveon_avg_sum_1", "__kaveon_avg_count_1")))
        self.assertEqual(answer, {
            "columns": ["surface", "Avg Latency (ms)"],
            "rows": [["Chat", 3.0], ["API", 2.5]],
            "query_id": "query-1", "duration_ms": 150,
        })


if __name__ == "__main__":
    unittest.main()
