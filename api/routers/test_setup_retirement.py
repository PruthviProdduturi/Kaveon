"""The setup router offers nothing to configure, because there is nothing.

These tests used to prove that retirement mode fenced the legacy metadata
control path before it could reach PostgreSQL -- a 409 from `/setup/test`,
`/admin/metadata/test`, `/admin/metadata/update` and `/admin/reset`. Those
endpoints are removed, so the fence has nothing to guard and a refusal is no
longer the thing worth asserting. Absence is: a route that does not exist
cannot reach a database that does not exist, and cannot be brought back by a
missing environment variable.

What is still worth pinning is the shape of the two endpoints Studio reads, and
that the module has no way back to a second system of record -- no pool, no
environment reader, no probe.
"""

import unittest

from routers import setup


class SetupSurfaceTests(unittest.TestCase):
    def test_only_the_two_endpoints_studio_reads_are_served(self):
        self.assertEqual(
            sorted(route.path for route in setup.router.routes),
            ["/admin/metadata", "/setup/status"],
        )

    def test_the_legacy_metadata_control_path_is_gone(self):
        """Each of these configured, probed or reset an external metadata
        database. Asserting the attributes are absent catches a reintroduction
        that an endpoint-path check alone would miss -- a helper can come back
        before a route does."""
        for name in (
            "setup_test", "setup_initialize",
            "admin_test_metadata", "admin_update_metadata", "admin_start_fresh",
            "_probe", "_read_env_vars", "_upsert_env", "_parse_odbc",
            "_resolve_fabric", "_assert_setup_mode",
            "_assert_external_metadata_enabled", "_is_metadata_configured",
            "_is_reset_allowed",
        ):
            with self.subTest(name=name):
                self.assertFalse(hasattr(setup, name), f"{name} came back")

    def test_the_module_cannot_reach_a_database_at_all(self):
        """No pool, and no PostgreSQL retirement switch: the authority is not
        conditional any more, so neither is this router."""
        self.assertFalse(hasattr(setup, "pool"))
        self.assertFalse(hasattr(setup, "postgresql_retirement_runtime"))

    def test_the_neon_migration_route_is_gone(self):
        """`POST /admin/fix-datasource-refs` rewrote `database_name` from
        'neondb' to 'kaveon' for a migration off a provider the platform no
        longer uses, and nothing in the product or the docs called it. It was an
        unparameterised UPDATE on `datasets` reachable by any administrator."""
        self.assertFalse(hasattr(setup, "fix_datasource_refs"))
        self.assertNotIn(
            "/admin/fix-datasource-refs",
            [route.path for route in setup.router.routes],
        )

    def test_status_reports_the_engine_as_the_authority(self):
        self.assertEqual(setup.setup_status(), {"status": "ok", "authority": "kaveondb"})

    def test_admin_metadata_describes_one_system_and_no_dsn(self):
        result = setup.admin_get_metadata(ctx={})
        self.assertEqual(result["db_type"], "kaveondb")
        self.assertEqual(result["authority"], "kaveondb")
        self.assertTrue(result["ui_configured"])
        # The Engine is reached over the bridge, so there is no host to report.
        # Publishing an empty string rather than omitting the key keeps the
        # connections page's shape stable.
        for empty in ("endpoint", "host", "port"):
            self.assertEqual(result[empty], "", f"{empty} should be empty")
        self.assertTrue(result["database"])
        self.assertTrue(result["schema"])


if __name__ == "__main__":
    unittest.main()
