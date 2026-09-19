import unittest
from unittest.mock import patch

from fastapi import HTTPException
from pydantic import ValidationError

from middleware.auth import UserContext
from routers import dlm as dlm_router
from services import engine_bridge

ANALYST = UserContext("analyst@example.com", "Analyst")
DATASET = {"id": "2", "dataset_name": "Product users", "database_name": "OpenSource",
           "schema_name": "kaveon_product", "table_name": "kaveon_events_users",
           "source": {"kind": "engine", "table_id": "local-opensource-kaveon_product-kaveon_events_users"},
           "columns": [], "metrics": [], "created_by": "analyst@example.com"}
SQL = "SELECT platform, COUNT(*) AS Users FROM kaveon_product.kaveon_events_users GROUP BY platform"


class ReproduceRouteTests(unittest.TestCase):
    def test_the_reproduce_block_runs_live_on_the_engine_with_the_forcing_settings(self):
        calls = []

        def execute(sql, catalog, actor, role, schema=None, timeout=60, settings=None):
            calls.append((sql, catalog, actor, role, schema, settings))
            return {"id": "q-7", "columns": [{"name": "platform"}, {"name": "Users"}],
                    "data": [["Desktop", 290187]], "elapsed_ms": 812,
                    "query_details": {"execution": {"mode": "distributed", "detail": "fragments"}}}

        with patch.object(dlm_router.dlm.datasets_svc, "get_dataset_by_id", return_value=DATASET), \
             patch.object(engine_bridge, "execute", side_effect=execute), \
             patch.object(engine_bridge, "table_version",
                          return_value={"source_version": {"identity_sha256": "f509", "kind": "file"}, "observed_at_ms": 1}), \
             patch.object(dlm_router.sql_execute_limiter, "check", lambda email: None):
            result = dlm_router.reproduce(dlm_router.ReproduceBody(dataset_id="2", sql=SQL), ANALYST)
        self.assertTrue(result["ok"])
        self.assertEqual(calls[0][0], SQL)
        self.assertEqual(calls[0][1:], ("OpenSource", "analyst@example.com", "Analyst", "kaveon_product",
                                        {"use_statistics": False, "result_cache": False}))
        self.assertEqual(result["rows"], [["Desktop", 290187]])
        self.assertEqual(result["columns"], ["platform", "Users"])
        evidence = result["evidence"]
        self.assertEqual((evidence["lane"], evidence["elapsed_ms"], evidence["rows"], evidence["query_id"]),
                         ("live", 812, 1, "q-7"))
        self.assertEqual(evidence["execution"]["mode"], "distributed")
        self.assertEqual(evidence["source_version"], {"identity_sha256": "f509", "kind": "file"})
        self.assertEqual(evidence["settings"], {"use_statistics": False, "result_cache": False})

    def test_only_a_read_only_statement_over_a_readable_dataset_runs(self):
        with patch.object(dlm_router.sql_execute_limiter, "check", lambda email: None):
            with self.assertRaises(HTTPException) as refused:
                dlm_router.reproduce(dlm_router.ReproduceBody(dataset_id="2", sql="DELETE FROM t"), ANALYST)
            self.assertEqual(refused.exception.status_code, 403)
            with patch.object(dlm_router.dlm.datasets_svc, "get_dataset_by_id", return_value=None):
                with self.assertRaises(HTTPException) as missing:
                    dlm_router.reproduce(dlm_router.ReproduceBody(dataset_id="2", sql=SQL), ANALYST)
            self.assertEqual(missing.exception.status_code, 404)
        with self.assertRaises(ValidationError):
            dlm_router.ReproduceBody(dataset_id="2", sql="")

    def test_an_engine_refusal_is_a_422_with_its_message(self):
        with patch.object(dlm_router.dlm.datasets_svc, "get_dataset_by_id", return_value=DATASET), \
             patch.object(engine_bridge, "execute", side_effect=HTTPException(422, {"message": "Engine query failed"})), \
             patch.object(dlm_router.sql_execute_limiter, "check", lambda email: None):
            with self.assertRaises(HTTPException) as failed:
                dlm_router.reproduce(dlm_router.ReproduceBody(dataset_id="2", sql=SQL), ANALYST)
        self.assertEqual((failed.exception.status_code, failed.exception.detail), (422, "Engine query failed"))


class CurationBodyTests(unittest.TestCase):
    def test_the_freshness_policy_and_approximate_flag_are_curatable(self):
        body = dlm_router.CurationBody(metrics={"Locales": {"approximate": False}}, freshness_policy="live")
        clean = dlm_router.dlm._sanitize_curation(body.model_dump())
        self.assertEqual(clean, {"metrics": {"Locales": {"approximate": False}}, "freshness_policy": "live"})
        with self.assertRaises(ValidationError):
            dlm_router.CurationBody(freshness_policy="sometimes")
        merged = dlm_router.dlm._merge_spec(
            {"metrics": {"Locales": {"additive": False, "approximate": True, "aliases": []}}, "dimensions": {},
             "value_aliases": {}, "freshness_policy": "cached"}, clean)
        self.assertFalse(merged["metrics"]["Locales"]["approximate"])
        self.assertEqual(merged["freshness_policy"], "live")


if __name__ == "__main__":
    unittest.main()
