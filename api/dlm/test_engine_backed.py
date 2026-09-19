"""The DLM over Engine tables: the dialect, generation from a table
definition and its shape, the settings chosen per policy, the label taken
from the Engine's `execution.mode`, freshness from `/version`, and the
evidence every answer carries on both source kinds."""
import contextlib
import json
import sys
import unittest
from contextlib import ExitStack
from types import SimpleNamespace
from unittest.mock import Mock, patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from fastapi import HTTPException

from dlm import engine
from dlm import engine_dialect as dialects
from dlm.test_ask_dialogue import PRODUCT_USERS, PRODUCT_USERS_INDEX, PRODUCT_USERS_DIMS
from services import engine_bridge

TABLE_ID = "local-opensource-kaveon_product-kaveon_events_users"
SHAPE = {
    "dimensions": [{"name": d, "cap": 40} for d in PRODUCT_USERS_DIMS],
    "measures": [{"column": "locale", "aggregates": ["count_distinct"]},
                 {"column": "user_id", "aggregates": ["count"]}],
}
TABLE = {
    "id": TABLE_ID, "schema_id": "local-opensource-kaveon_product", "name": "kaveon_events_users",
    "revision": 4, "location": "kaveon_product/kaveon_events_users/combined-v1.parquet",
    "access": "Shortcut", "format": "Parquet", "lifecycle": "Active",
    "columns": [{"name": "user_id", "data_type": "Int64", "nullable": True},
                {"name": "locale", "data_type": "Utf8", "nullable": True}]
               + [{"name": d, "data_type": "Utf8", "nullable": True} for d in PRODUCT_USERS_DIMS],
    "shape": SHAPE,
}
SCHEMA = {"id": "local-opensource-kaveon_product", "catalog_id": "local-opensource", "name": "kaveon_product",
          "revision": 2, "lifecycle": "Active"}
CATALOG = {"id": "local-opensource", "name": "OpenSource", "revision": 2, "adapter": "Native",
           "storage": {"Local": {"base_path": "/data"}}, "credential": None, "lifecycle": "Active"}
VERSION_A = {"identity_sha256": "f509311d2722feb3e57a20d7ac6eaf0d6668a2b083125022a16f52c70e7dac58", "kind": "file"}
VERSION_B = {"identity_sha256": "0b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c", "kind": "file"}
BOUND_DATASET = {**PRODUCT_USERS, "source": {"kind": "engine", "table_id": TABLE_ID}}
COLUMNS = [{"column_name": "order_date", "data_type": "date"},
           {"column_name": "placed_at", "data_type": "timestamp"},
           {"column_name": "year", "data_type": "integer"},
           {"column_name": "event_date", "data_type": "varchar"}]


def _record(mode, detail=None, approximate=None, source_version=VERSION_A):
    execution = {"mode": mode, "detail": detail or mode}
    if mode == "context":
        execution["source_version"] = source_version
        execution["current_source_version"] = source_version
    if approximate:
        execution["approximate"] = approximate
    return {"id": "q-1", "state": "FINISHED", "execution": execution, "elapsed_ms": 3, "scans": []}


def _result(columns, rows, record):
    return {"id": "q-1", "columns": [{"name": c, "type": "UInt64"} for c in columns], "data": rows,
            "elapsed_ms": record["elapsed_ms"], "query_details": record}


class DialectTests(unittest.TestCase):
    def test_identifiers_and_relations(self):
        self.assertEqual(dialects.POSTGRESQL.relation("sales", "orders"), '"sales"."orders"')
        self.assertEqual(dialects.ENGINE.relation("sales", "orders"), "sales.orders")
        self.assertEqual(dialects.ENGINE.ident("region"), "region")
        self.assertEqual(dialects.ENGINE.ident("year"), '"year"')          # reserved word stays a column
        self.assertEqual(dialects.ENGINE.ident("team size"), '"team size"')
        self.assertEqual(dialects.ENGINE.expression('COUNT(DISTINCT "locale")'), "COUNT(DISTINCT locale)")
        self.assertEqual(dialects.POSTGRESQL.expression('COUNT(DISTINCT "locale")'), 'COUNT(DISTINCT "locale")')

    def test_year_predicates_by_column_type(self):
        pg, en = dialects.POSTGRESQL, dialects.ENGINE
        self.assertEqual(pg.year_predicate("order_date", 2026, COLUMNS),
                         '"order_date" >= \'2026-01-01\' AND "order_date" < \'2027-01-01\'')
        self.assertEqual(en.year_predicate("order_date", 2026, COLUMNS),
                         "order_date >= DATE '2026-01-01' AND order_date < DATE '2027-01-01'")
        self.assertEqual(en.year_predicate("placed_at", 2026, COLUMNS), "EXTRACT(YEAR FROM placed_at) = 2026")
        self.assertEqual(en.year_predicate("year", 2026, COLUMNS), '"year" = 2026')
        self.assertEqual(pg.year_predicate("year", 2026, COLUMNS), '"year" = 2026')
        self.assertEqual(en.year_predicate("event_date", 2026, COLUMNS),
                         "event_date >= '2026-01-01' AND event_date < '2027-01-01'")

    def test_windows_and_relative_time(self):
        en = dialects.ENGINE
        self.assertEqual(en.window_predicate("placed_at", "2026-07-01", "2026-08-01", COLUMNS),
                         "EXTRACT(EPOCH FROM placed_at) >= 1782864000 AND EXTRACT(EPOCH FROM placed_at) < 1785542400")
        self.assertEqual(en.relative_predicate("order_date", "'2026-09-12'", "2026-09-12", None, COLUMNS),
                         "order_date >= DATE '2026-09-12'")
        self.assertEqual(dialects.POSTGRESQL.relative_predicate("order_date", "CURRENT_DATE - INTERVAL '1 day'",
                                                                "2026-09-18", "2026-09-19", COLUMNS),
                         "\"order_date\" >= CURRENT_DATE - INTERVAL '1 day'")

    def test_assembly_in_both_dialects_and_the_cube_shape(self):
        common = dict(schema="kaveon_product", fact="kaveon_events_users", metric_expr="COUNT(*)",
                      metric_name="Users", group_cols=["country"], time_group=None,
                      filters=[{"column": "region", "value": "Europe"}], columns=[], limit_n=5, sort_asc=False)
        self.assertEqual(
            dialects.assemble(dialects.POSTGRESQL, **common),
            'SELECT "country", COUNT(*) AS "Users" FROM "kaveon_product"."kaveon_events_users" '
            'WHERE "region" = \'Europe\' GROUP BY "country" ORDER BY "Users" DESC LIMIT 5')
        self.assertEqual(
            dialects.assemble(dialects.ENGINE, **common),
            "SELECT country, COUNT(*) AS Users FROM kaveon_product.kaveon_events_users "
            "WHERE region = 'Europe' GROUP BY country ORDER BY Users DESC LIMIT 5")
        # ranked=False is the statement the cube answers: no ORDER BY, no LIMIT.
        self.assertEqual(
            dialects.assemble(dialects.ENGINE, ranked=False, **common),
            "SELECT country, COUNT(*) AS Users FROM kaveon_product.kaveon_events_users "
            "WHERE region = 'Europe' GROUP BY country")
        self.assertEqual(
            dialects.assemble(dialects.ENGINE, **{**common, "filters": [{"column": "country", "value": "Côte d'Ivoire"}]}),
            "SELECT country, COUNT(*) AS Users FROM kaveon_product.kaveon_events_users "
            "WHERE country = 'Côte d''Ivoire' GROUP BY country ORDER BY Users DESC LIMIT 5")


class GenerationTests(unittest.TestCase):
    def test_generation_compiles_from_the_engine_definition_and_its_shape(self):
        statements = []

        def execute(sql, catalog, actor, role, schema=None, timeout=60, settings=None):
            statements.append(sql)
            if sql.startswith("SELECT COUNT(*) AS row_count"):
                return {"columns": [{"name": "row_count"}], "data": [[3000000]]}
            column = sql.split(" AS v")[0].split("SELECT ")[1]
            return {"columns": [{"name": "v"}, {"name": "c"}],
                    "data": [[f"{column}-a", 2], [None, 1], [f"{column}-b", 5]]}

        persisted = {}
        stubs = {"_analyze_tables": None, "_usage_rollup": {}, "_persist_value_index": None,
                 "_upsert_router": None, "_curate_linked_dashboards": 0}
        with ExitStack() as stack:
            stack.enter_context(patch.object(engine, "ensure_tables", lambda: None))
            stack.enter_context(patch.object(engine.datasets_svc, "get_dataset_by_id", return_value=BOUND_DATASET))
            stack.enter_context(patch.object(engine.profiler, "build_context", return_value={"supported": False}))
            stack.enter_context(patch.object(engine_bridge, "table_definition_by_id", return_value=TABLE))
            stack.enter_context(patch.object(engine_bridge, "schema_definition", return_value=SCHEMA))
            stack.enter_context(patch.object(engine_bridge, "catalog_definition", return_value=CATALOG))
            stack.enter_context(patch.object(engine_bridge, "table_version",
                                             return_value={"table_id": TABLE_ID, "source_version": VERSION_A,
                                                           "observed_at_ms": 1789808661372}))
            stack.enter_context(patch.object(engine_bridge, "execute", side_effect=execute))
            stack.enter_context(patch.object(engine.meta, "query_one", return_value=None))
            stack.enter_context(patch.object(engine.meta, "execute", lambda *a, **k: None))
            transaction = SimpleNamespace(execute=Mock(return_value=1))
            stack.enter_context(patch.object(engine.meta, "transaction", return_value=contextlib.nullcontext(transaction)))
            stack.enter_context(patch("services.dlm_definition_mutations.publish_ready"))
            stack.enter_context(patch("services.dlm_compiled_artifact.publish", return_value=None))
            for name, value in stubs.items():
                stack.enter_context(patch.object(engine, name, lambda *a, _v=value, **k: _v))

            def upsert(dataset_id, manifest, stats_rollup, usage_rollup, source_hash, status):
                persisted.update(manifest=manifest, stats=stats_rollup, status=status)
            stack.enter_context(patch.object(engine, "_upsert_artifact", upsert))
            stack.enter_context(patch.object(engine, "_effective_spec",
                                             lambda i: persisted["manifest"]["context_spec"]))
            precompute = stack.enter_context(patch.object(engine, "_precompute_answers", return_value=0))
            result = engine.generate_dlm("2", force=True)

        self.assertTrue(result["ok"] and result["status"] == "ready", result)
        precompute.assert_not_called()                      # no warehouse cuboids for an Engine table
        spec = persisted["manifest"]["context_spec"]
        self.assertEqual(spec["freshness_policy"], "cached")
        self.assertFalse(spec["metrics"]["Locales"]["additive"])
        self.assertTrue(spec["metrics"]["Locales"]["approximate"])
        self.assertNotIn("approximate", spec["metrics"]["Users"])
        self.assertEqual(persisted["manifest"]["engine"]["shape"], SHAPE)
        stats = persisted["stats"]
        self.assertEqual(stats["engine"]["table_id"], TABLE_ID)
        self.assertEqual(stats["engine"]["source_version"], VERSION_A)
        self.assertEqual(stats["engine"]["table"], "OpenSource.kaveon_product.kaveon_events_users")
        self.assertEqual(stats["watermark"]["source_version"], VERSION_A)
        self.assertEqual(stats["row_counts"], {"kaveon_events_users": 3000000})
        # Each declared dimension's values come from the cube-shaped statement:
        # plain grouped count, no null test, no ORDER BY, no LIMIT.
        value_scans = [s for s in statements if " AS v, COUNT(*) AS c FROM " in s]
        self.assertEqual(len(value_scans), len(PRODUCT_USERS_DIMS))
        for scan in value_scans:
            self.assertNotIn("ORDER BY", scan)
            self.assertNotIn("LIMIT", scan)
            self.assertNotIn("IS NOT NULL", scan)
            self.assertIn("FROM kaveon_product.kaveon_events_users GROUP BY", scan)

    def test_a_cube_dimension_scan_drops_the_null_key_and_orders_by_support(self):
        with patch.object(engine, "_execute_dataset_query",
                          return_value={"rows_objects": [{"v": "Web", "c": 1}, {"v": None, "c": 9}, {"v": "Desktop", "c": 4}]}) as run:
            pairs = engine._scan_distinct("OpenSource", "kaveon_product", "kaveon_events_users", "platform", 1000,
                                          cube_dim=True)
        self.assertEqual(pairs, [("Desktop", 4.0), ("Web", 1.0)])
        self.assertEqual(run.call_args[0][0],
                         "SELECT platform AS v, COUNT(*) AS c FROM kaveon_product.kaveon_events_users GROUP BY platform")


class EngineAskHarness(ExitStack):
    """ask() over the bound dataset with the Engine's answer scripted."""
    def __init__(self, result, spec_overrides=None, engine_meta=True):
        super().__init__()
        self.result = result
        self.spec_overrides = spec_overrides or {}
        self.engine_meta = engine_meta
        self.calls = []

    def __enter__(self):
        super().__enter__()
        suggested = engine._suggest_spec(BOUND_DATASET["columns"], BOUND_DATASET["metrics"], shape=SHAPE, engine=True)
        spec = engine._merge_spec(suggested, self.spec_overrides)

        def resolve(dataset_id, term, limit=5, exact_only=False, actor=None, role="Viewer"):
            norm = engine._VALUE_ALIASES.get(engine._normalize(term), engine._normalize(term))
            rows = [r for r in PRODUCT_USERS_INDEX if r["value_norm"] == norm]
            return [engine._value_hit(r) for r in rows[:limit]]

        def execute(sql, catalog, actor, role, schema=None, timeout=60, settings=None):
            self.calls.append({"sql": sql, "catalog": catalog, "actor": actor, "role": role,
                               "schema": schema, "settings": settings})
            if isinstance(self.result, Exception):
                raise self.result
            return self.result

        stats = {"row_counts": {"kaveon_events_users": 3000000},
                 "engine": {"table_id": TABLE_ID, "table": "OpenSource.kaveon_product.kaveon_events_users",
                            "shape": SHAPE, "source_version": VERSION_A}} if self.engine_meta else {}
        self.enter_context(patch.object(engine, "ensure_tables", lambda: None))
        self.enter_context(patch.object(engine, "route", lambda q, limit=1: [{"dataset_id": "2", "score": 16.0}]))
        self.enter_context(patch.object(engine.datasets_svc, "get_dataset_by_id",
                                        lambda i, *a, **k: BOUND_DATASET if str(i) == "2" else None))
        self.enter_context(patch.object(engine, "_effective_spec", lambda i: spec))
        self.enter_context(patch.object(engine, "resolve_value", resolve))
        self.enter_context(patch.object(engine, "_indexed_values", lambda i: list(PRODUCT_USERS_INDEX)))
        self.enter_context(patch.object(engine, "_vocabulary_hit", lambda q: True))
        self.enter_context(patch.object(engine, "_artifact_stats", lambda i: stats))
        self.enter_context(patch.object(engine, "_context_answer",
                                        side_effect=AssertionError("warehouse context cell reached")))
        self.enter_context(patch.object(engine, "_native_catalog",
                                        side_effect=AssertionError("catalog registry reached")))
        self.enter_context(patch.object(engine_bridge, "execute", execute))
        self.enter_context(patch.object(engine_bridge, "table_version",
                                        return_value={"table_id": TABLE_ID, "source_version": VERSION_A,
                                                      "observed_at_ms": 1}))
        return self


class EngineAskTests(unittest.TestCase):
    def test_a_covered_breakdown_is_one_cube_shaped_statement_labelled_from_the_record(self):
        record = _record("context", "cube at file (f509311d2722)")
        result = _result(["platform", "Users"], [["Desktop", 290187], ["Web", 241838], ["Mobile", 242540]], record)
        with EngineAskHarness(result) as harness:
            answer = engine.ask("users by platform in Europe", actor="analyst@example.com", role="Analyst")
        self.assertTrue(answer["ok"], answer)
        call = harness.calls[0]
        self.assertEqual(call["sql"], "SELECT platform, COUNT(*) AS Users FROM kaveon_product.kaveon_events_users "
                                      "WHERE region = 'Europe' GROUP BY platform")
        self.assertEqual((call["catalog"], call["schema"], call["actor"], call["role"]),
                         ("OpenSource", "kaveon_product", "analyst@example.com", "Analyst"))
        self.assertEqual(call["settings"], {"result_cache": True, "use_statistics": True})
        self.assertTrue(answer["from_context"])
        self.assertEqual(answer["route"], "context")
        self.assertFalse(answer["approx"])
        self.assertTrue(answer["engine"] and answer["executed"])
        # The DLM ranks the cube's cells itself: descending by the measure.
        self.assertEqual([r[0] for r in answer["rows"]], ["Desktop", "Mobile", "Web"])
        evidence = answer["evidence"]
        self.assertEqual(evidence["lane"], "context")
        self.assertEqual(evidence["execution"], record["execution"])
        self.assertEqual(evidence["source_version"], VERSION_A)
        self.assertEqual(evidence["source"], {"kind": "engine", "table_id": TABLE_ID, "catalog": "OpenSource",
                                              "schema": "kaveon_product", "table": "kaveon_events_users"})
        self.assertEqual(evidence["dataset"], {"id": "2", "name": "Product users"})
        self.assertEqual(evidence["rows"], 3)
        self.assertEqual(evidence["query_id"], "q-1")
        self.assertEqual(evidence["reproduce"], {"sql": call["sql"], "database": "OpenSource", "schema": "kaveon_product",
                                                 "engine": True,
                                                 "settings": {"use_statistics": False, "result_cache": False}})
        self.assertEqual(evidence["settings"], call["settings"])

    def test_a_read_statement_is_live_and_a_cached_one_is_cache(self):
        result = _result(["Users"], [[3000000]], _record("distributed", "fragments"))
        with EngineAskHarness(result):
            live = engine.ask("how many users", actor="analyst@example.com", role="Analyst")
        self.assertFalse(live["from_context"])
        self.assertEqual((live["route"], live["evidence"]["lane"]), ("live", "live"))
        self.assertEqual(live["evidence"]["execution"]["mode"], "distributed")
        # A read record names no version: the answer reflects the source now.
        self.assertEqual(live["evidence"]["source_version"], VERSION_A)
        cached = _result(["Users"], [[3000000]], _record("cache", "hit"))
        with EngineAskHarness(cached):
            hit = engine.ask("how many users", actor="analyst@example.com", role="Analyst")
        self.assertEqual((hit["from_context"], hit["route"], hit["evidence"]["lane"]), (False, "cache", "cache"))

    def test_approximate_is_sent_only_for_a_metric_the_spec_marks_and_labels_the_answer(self):
        note = [{"function": "COUNT", "argument": "locale", "sketch": "hyperloglog p=12",
                 "error": 0.01625, "error_kind": "relative_standard_error"}]
        result = _result(["platform", "Locales"], [["Desktop", 7], ["Mobile", 7], ["Web", 7]],
                         _record("context", "cube at file (f509311d2722) (hyperloglog p=12)", note))
        with EngineAskHarness(result) as harness:
            answer = engine.ask("locales by platform in Europe", actor="analyst@example.com", role="Analyst")
        self.assertEqual(harness.calls[0]["settings"], {"result_cache": True, "use_statistics": True, "approximate": True})
        self.assertTrue(answer["from_context"] and answer["approx"])
        self.assertEqual(answer["evidence"]["execution"]["approximate"], note)
        # Curation turns the approximation off: the exact COUNT(DISTINCT) runs on the rows.
        overrides = {"metrics": {"Locales": {"approximate": False}}}
        exact = _result(["platform", "Locales"], [["Desktop", 7]], _record("distributed"))
        with EngineAskHarness(exact, spec_overrides=overrides) as harness:
            engine.ask("locales by platform in Europe", actor="analyst@example.com", role="Analyst")
        self.assertEqual(harness.calls[0]["settings"], {"result_cache": True, "use_statistics": True})

    def test_the_freshness_policy_decides_the_result_cache(self):
        result = _result(["Users"], [[3000000]], _record("context", "statistics at file (f509311d2722)"))
        with EngineAskHarness(result, spec_overrides={"freshness_policy": "live"}) as harness:
            engine.ask("how many users", actor="analyst@example.com", role="Analyst")
        self.assertEqual(harness.calls[0]["settings"], {"result_cache": False, "use_statistics": True})

    def test_a_ranking_over_an_undeclared_axis_keeps_its_order_and_limit_on_the_statement(self):
        undeclared = {**BOUND_DATASET,
                      "columns": BOUND_DATASET["columns"] + [{"table_name": "kaveon_events_users", "column_name": "device",
                                                              "data_type": "varchar", "is_dimension": True}]}
        result = _result(["device", "Users"], [["Tablet", 9]], _record("distributed"))
        with EngineAskHarness(result) as harness, \
             patch.object(engine.datasets_svc, "get_dataset_by_id", lambda i, *a, **k: undeclared):
            answer = engine.ask("top 3 devices by users", actor="analyst@example.com", role="Analyst")
        self.assertTrue(answer["ok"], answer)
        self.assertTrue(harness.calls[0]["sql"].endswith("GROUP BY device ORDER BY Users DESC LIMIT 3"))

    def test_a_viewer_runs_as_the_service_principal(self):
        result = _result(["Users"], [[3000000]], _record("context"))
        with EngineAskHarness(result) as harness:
            answer = engine.ask("how many users", actor="viewer@example.com", role="Viewer")
        self.assertTrue(answer["ok"], answer)
        self.assertEqual((harness.calls[0]["actor"], harness.calls[0]["role"]), ("kaveon-system", "Analyst"))
        self.assertEqual(answer["evidence"]["principal"], "kaveon-system")

    def test_an_engine_failure_is_an_answer_with_the_message_and_the_statement(self):
        failure = HTTPException(422, {"message": "Engine query failed", "query_id": "q-9",
                                      "engine_details": {"execution": {"mode": "distributed"}}})
        with EngineAskHarness(failure):
            answer = engine.ask("how many users", actor="analyst@example.com", role="Analyst")
        self.assertFalse(answer["ok"])
        self.assertEqual(answer["reason"], "query_failed")
        self.assertEqual(answer["message"], "Engine query failed")
        self.assertEqual(answer["evidence"]["query_id"], "q-9")
        self.assertTrue(answer["sql"].startswith("SELECT COUNT(*) AS Users FROM kaveon_product.kaveon_events_users"))


class FreshnessTests(unittest.TestCase):
    def _artifact(self):
        return {"built_at": "2026-09-19T00:00:00", "status": "ready", "manifest": json.dumps({
            "fact_table": "kaveon_events_users", "schema": "kaveon_product"}),
            "stats_rollup": json.dumps({"row_counts": {"kaveon_events_users": 3000000},
                                        "engine": {"table_id": TABLE_ID, "source_version": VERSION_A}})}

    def test_an_unchanged_source_version_is_fresh(self):
        with patch.object(engine, "ensure_tables", lambda: None), \
             patch.object(engine.meta, "query_one", return_value=self._artifact()), \
             patch.object(engine_bridge, "table_version",
                          return_value={"table_id": TABLE_ID, "source_version": VERSION_A, "observed_at_ms": 5}), \
             patch("dlm.validity._live_change", side_effect=AssertionError("warehouse counter reached")):
            fresh = engine.check_freshness("2")
        self.assertTrue(fresh["fresh"])
        self.assertFalse(fresh["data_modified"])
        self.assertEqual(fresh["recommendation"], "use_context")
        self.assertEqual(fresh["signal"], "engine_source_version")
        self.assertEqual((fresh["source_version"], fresh["current_source_version"], fresh["observed_at_ms"]),
                         (VERSION_A, VERSION_A, 5))

    def test_a_moved_source_version_is_a_change_of_the_half_fraction(self):
        with patch.object(engine, "ensure_tables", lambda: None), \
             patch.object(engine.meta, "query_one", return_value=self._artifact()), \
             patch.object(engine_bridge, "table_version",
                          return_value={"table_id": TABLE_ID, "source_version": VERSION_B, "observed_at_ms": 6}):
            stale = engine.check_freshness("2")
        self.assertTrue(stale["data_modified"])
        self.assertEqual(stale["recommendation"], "rebuild")
        self.assertLess(stale["score"], 0.51)
        self.assertEqual(stale["current_source_version"], VERSION_B)

    def test_an_unreachable_engine_leaves_the_score_on_time_alone(self):
        with patch.object(engine, "ensure_tables", lambda: None), \
             patch.object(engine.meta, "query_one", return_value=self._artifact()), \
             patch.object(engine_bridge, "table_version", side_effect=HTTPException(503, "down")):
            result = engine.check_freshness("2")
        self.assertFalse(result["data_modified"])
        self.assertIsNone(result["current_source_version"])


class WarehouseEvidenceTests(unittest.TestCase):
    def test_a_context_answer_reflects_the_change_counter_the_artifact_was_built_against(self):
        from dlm.test_ask_dialogue import ProductUsersHarness
        snapshot = {"kind": "postgresql_change_counter", "table": "kaveon_events_users", "row_count": 3000000,
                    "mods_since_analyze": 0, "last_analyze": "2026-09-19T00:00:00", "observed_at": "2026-09-19T00:00:01"}
        with ProductUsersHarness(), patch.object(engine, "_artifact_stats", lambda i: {"change_counter": snapshot}):
            answer = engine.ask("top 2 countries by users")
        self.assertTrue(answer["from_context"], answer)
        evidence = answer["evidence"]
        self.assertEqual(evidence["lane"], "context")
        self.assertEqual(evidence["source"], {"kind": "warehouse", "database": "OpenSource", "schema": "kaveon_product",
                                              "table": "kaveon_events_users"})
        self.assertEqual(evidence["source_version"], snapshot)
        self.assertEqual(evidence["sql"], 'SELECT "country", COUNT(*) AS "Users" FROM "kaveon_product"."kaveon_events_users" '
                                          'GROUP BY "country" ORDER BY "Users" DESC LIMIT 2')
        self.assertEqual(evidence["rows"], 2)
        self.assertIsNone(evidence["execution"])
        self.assertEqual(evidence["elapsed_ms"], answer["duration_ms"])
        self.assertEqual(evidence["reproduce"], {"sql": evidence["sql"], "database": "OpenSource",
                                                 "schema": "kaveon_product", "engine": False, "settings": None})

    def test_a_live_answer_reflects_the_counter_now_and_leaves_rows_to_the_client(self):
        from dlm.test_ask_dialogue import ProductUsersHarness
        now = {"kind": "postgresql_change_counter", "table": "kaveon_events_users", "mods_since_analyze": 12}
        with ProductUsersHarness(), patch.object(engine, "_change_counter", lambda *a: now):
            answer = engine.ask("users by country in Europe for desktop")
        self.assertFalse(answer["from_context"], answer)
        evidence = answer["evidence"]
        self.assertEqual(evidence["lane"], "live")
        self.assertEqual(evidence["source_version"], now)
        self.assertIsNone(evidence["rows"])
        self.assertEqual(evidence["reproduce"]["sql"], evidence["sql"])


if __name__ == "__main__":
    unittest.main()
