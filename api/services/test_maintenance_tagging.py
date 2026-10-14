"""Maintenance statements name themselves.

A cube build and a chart share the Engine's one `default` resource group, so a
build takes the interactive share with it. The Engine can separate them --
`resource_groups` selectors match on `client_tag` -- and this is the half that
belongs to the API: the statement has to say what it is before anything can be
bound away from the interactive pool.
"""

import sys
import unittest
from types import SimpleNamespace
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from services import engine_bridge


class MaintenanceTaggingTests(unittest.TestCase):
    def test_an_interactive_statement_is_not_tagged_as_maintenance(self):
        payload = engine_bridge._statement_payload(
            "SELECT 1", "cat", "sch", "kaveon-api:abc", None)
        self.assertEqual(payload["client_tags"], ["kaveon-api:abc"])

    def test_analyze_names_itself_maintenance_and_keeps_its_cancellation_tag(self):
        """The per-statement tag must survive, because `cancel_tagged` is how a
        caller that gives up stops the scan rather than leaving it running."""
        captured = {}

        def fake_send(method, path, token, actor, **kwargs):
            captured.update(kwargs.get("payload") or {})
            return SimpleNamespace(status_code=200, is_success=True,
                                   json=lambda: {"data": [[]], "columns": []})

        with patch.object(engine_bridge, "_send", side_effect=fake_send), \
             patch.object(engine_bridge, "_response_json",
                          return_value={"data": [[]], "columns": []}):
            engine_bridge.analyze_table("cat", "sch", "tbl", "a@b.c", "Admin", cube=True)

        tags = captured["client_tags"]
        self.assertIn(engine_bridge.MAINTENANCE_CLIENT_TAG, tags)
        self.assertTrue(any(t.startswith("kaveon-api:") for t in tags),
                        "the cancellation tag must still be present")
        self.assertIn("cube = true", captured["query"])


if __name__ == "__main__":
    unittest.main()
