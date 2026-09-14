import contextlib
import os
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from services import charts, dashboards


class Transaction:
    def __init__(self):
        self.statements = []
        self.committed = False

    def execute(self, sql, params=None):
        self.statements.append((" ".join(sql.split()), params or []))
        return 1


@contextlib.contextmanager
def atomic(transaction):
    try:
        yield transaction
    except Exception:
        transaction.committed = False
        raise
    else:
        transaction.committed = True


class ChartDashboardOutboxTests(unittest.TestCase):
    def test_chart_create_outbox_failure_rolls_back_source_write(self):
        transaction = Transaction()
        data = {"name": "Trips", "dataset_id": "7", "chart_type": "bar"}
        with patch.dict(os.environ, {"KAVEON_CHART_OUTBOX_ENABLED": "true"}, clear=True), \
             patch.object(charts, "_chart_schema", return_value="modern"), \
             patch.object(charts.db, "transaction", return_value=atomic(transaction)), \
             patch.object(charts, "_enqueue", side_effect=RuntimeError("outbox failed")), \
             patch.object(charts, "get_chart_by_id") as read:
            with self.assertRaisesRegex(RuntimeError, "outbox failed"):
                charts.create_chart(data, "owner@example.com")
        self.assertFalse(transaction.committed)
        self.assertEqual(len(transaction.statements), 1)
        read.assert_not_called()

    def test_dashboard_create_outbox_failure_rolls_back_source_write(self):
        transaction = Transaction()
        with patch.dict(os.environ, {"KAVEON_DASHBOARD_OUTBOX_ENABLED": "true"}, clear=True), \
             patch.object(dashboards.db, "transaction", return_value=atomic(transaction)), \
             patch.object(dashboards, "_enqueue", side_effect=RuntimeError("outbox failed")), \
             patch.object(dashboards, "get_dashboard_by_id") as read:
            with self.assertRaisesRegex(RuntimeError, "outbox failed"):
                dashboards.create_dashboard({"name": "Executive"}, "owner@example.com")
        self.assertFalse(transaction.committed)
        self.assertEqual(len(transaction.statements), 1)
        read.assert_not_called()

    def test_outbox_flags_are_default_off(self):
        with patch.dict(os.environ, {}, clear=True), \
             patch.object(charts, "_chart_schema", return_value="modern"), \
             patch.object(charts.db, "execute") as chart_write, \
             patch.object(charts.db, "transaction") as chart_transaction, \
             patch.object(charts, "get_chart_by_id", return_value={"id": "chart"}):
            charts.create_chart({"name": "Trips", "dataset_id": "7", "chart_type": "bar"}, "owner")
        chart_write.assert_called_once()
        chart_transaction.assert_not_called()

        with patch.dict(os.environ, {}, clear=True), \
             patch.object(dashboards.db, "execute") as dashboard_write, \
             patch.object(dashboards.db, "transaction") as dashboard_transaction, \
             patch.object(dashboards, "get_dashboard_by_id", return_value={"id": "dashboard"}):
            dashboards.create_dashboard({"name": "Executive"}, "owner")
        dashboard_write.assert_called_once()
        dashboard_transaction.assert_not_called()

    def test_enabled_create_captures_event_before_commit(self):
        for service, flag, create, data, schema in (
            (charts, "KAVEON_CHART_OUTBOX_ENABLED", charts.create_chart,
             {"name": "Trips", "dataset_id": "7", "chart_type": "bar"}, True),
            (dashboards, "KAVEON_DASHBOARD_OUTBOX_ENABLED", dashboards.create_dashboard,
             {"name": "Executive"}, False),
        ):
            with self.subTest(service=service.__name__):
                transaction = Transaction()
                observed = []
                patches = [
                    patch.dict(os.environ, {flag: "true"}, clear=True),
                    patch.object(service.db, "transaction", return_value=atomic(transaction)),
                    patch.object(service, "_enqueue", side_effect=lambda *args: observed.append((args[0] is transaction, transaction.committed))),
                ]
                if schema:
                    patches.append(patch.object(charts, "_chart_schema", return_value="modern"))
                with contextlib.ExitStack() as stack:
                    for item in patches:
                        stack.enter_context(item)
                    stack.enter_context(patch.object(service, "get_chart_by_id" if schema else "get_dashboard_by_id",
                                                     return_value={"id": "created"}))
                    create(data, "owner@example.com")
                self.assertTrue(transaction.committed)
                self.assertEqual(observed, [(True, False)])


if __name__ == "__main__":
    unittest.main()
