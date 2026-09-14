"""Retirement mode must never reach the legacy metadata database control path."""

import os
import unittest
from unittest.mock import patch

from fastapi import HTTPException

from models.setup import SetupConnectionBody
from routers import setup


class SetupRetirementTests(unittest.TestCase):
    def setUp(self):
        self.mode = patch.dict(os.environ, {"KAVEON_POSTGRESQL_RETIREMENT_MODE": "true"})
        self.mode.start()

    def tearDown(self):
        self.mode.stop()

    def test_status_reports_kaveondb_without_database_probe(self):
        with patch.object(setup.pool, "execute_query", side_effect=AssertionError("PostgreSQL reached")):
            self.assertEqual(
                setup.setup_status(),
                {"status": "ok", "authority": "kaveondb"},
            )

    def test_admin_metadata_reports_kaveondb_without_reading_legacy_config(self):
        with patch.object(setup, "_read_env_vars", side_effect=AssertionError("legacy config reached")):
            result = setup.admin_get_metadata(ctx={})
        self.assertEqual(result["db_type"], "kaveondb")
        self.assertEqual(result["authority"], "kaveondb")
        self.assertTrue(result["ui_configured"])

    def test_setup_probe_is_fenced_before_pool_access(self):
        request = SetupConnectionBody(db_type="postgresql", database="metadata", host="localhost")
        with patch.object(setup, "_probe", side_effect=AssertionError("PostgreSQL reached")):
            with self.assertRaises(HTTPException) as raised:
                setup.setup_test(request)
        self.assertEqual(raised.exception.status_code, 409)
        self.assertEqual(raised.exception.detail["code"], "postgresql_retired")

    def test_admin_mutations_are_fenced_before_side_effects(self):
        request = SetupConnectionBody(db_type="postgresql", database="metadata", host="localhost")
        calls = (
            lambda: setup.admin_test_metadata(request, ctx={}),
            lambda: setup.admin_update_metadata(request, ctx={}),
            lambda: setup.admin_start_fresh(ctx={}),
            lambda: setup.fix_datasource_refs(ctx={}),
        )
        with patch.object(setup, "_probe", side_effect=AssertionError("PostgreSQL reached")), \
             patch.object(setup, "_upsert_env", side_effect=AssertionError("config mutation reached")), \
             patch.object(setup.pool, "get_connection_pool", side_effect=AssertionError("PostgreSQL reached")):
            for call in calls:
                with self.subTest(call=call):
                    with self.assertRaises(HTTPException) as raised:
                        call()
                    self.assertEqual(raised.exception.status_code, 409)


if __name__ == "__main__":
    unittest.main()
