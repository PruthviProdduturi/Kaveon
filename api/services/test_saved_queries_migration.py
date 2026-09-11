import contextlib
import os
import sys
import unittest
from types import SimpleNamespace
from unittest.mock import patch

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from services import saved_queries


def source_row(query_id="query-1"):
    return {"id": query_id, "name": "Trips", "description": None,
            "sql_text": "SELECT 1", "created_by": "owner@example.test",
            "created_at": "2026-01-01", "modified_by": "owner@example.test",
            "modified_at": "2026-01-02"}


class Transaction:
    def __init__(self, rows=None, deleted=1):
        self.rows = list(rows or [])
        self.deleted = deleted
        self.calls = []

    def query_one(self, sql, params=None):
        self.calls.append(("query", " ".join(sql.split()), params))
        return self.rows.pop(0) if self.rows else None

    def execute(self, sql, params=None):
        self.calls.append(("execute", " ".join(sql.split()), params))
        return self.deleted


class SavedQueryMutationTests(unittest.TestCase):
    def setUp(self):
        self.outbox_env=patch.dict(os.environ,{"KAVEON_SAVED_QUERY_OUTBOX_ENABLED":"true"});self.outbox_env.start();self.addCleanup(self.outbox_env.stop)

    def test_create_mutation_and_event_share_transaction(self):
        transaction = Transaction([source_row()])
        with patch.object(saved_queries.db, "transaction",
                          return_value=contextlib.nullcontext(transaction)), \
             patch.object(saved_queries.product_outbox, "enqueue") as enqueue:
            result = saved_queries.create_saved_query(
                {"name": "Trips", "sql": "SELECT 1"}, "owner@example.test")
        self.assertEqual(result["id"], "query-1")
        self.assertIn("RETURNING id", transaction.calls[0][1])
        self.assertIs(enqueue.call_args.args[0], transaction)
        self.assertEqual(enqueue.call_args.kwargs["payload"]["sql"], "SELECT 1")

    def test_create_outbox_failure_escapes_transaction_boundary(self):
        transaction = Transaction([source_row()])
        with patch.object(saved_queries.db, "transaction",
                          return_value=contextlib.nullcontext(transaction)), \
             patch.object(saved_queries.product_outbox, "enqueue",
                          side_effect=RuntimeError("outbox failed")):
            with self.assertRaisesRegex(RuntimeError, "outbox failed"):
                saved_queries.create_saved_query(
                    {"name": "Trips", "sql": "SELECT 1"}, "owner@example.test")

    def test_update_locks_then_returns_canonical_event(self):
        transaction = Transaction([
            {"id": "query-1", "created_by": "owner@example.test"},
            source_row(),
        ])
        with patch.object(saved_queries.db, "transaction",
                          return_value=contextlib.nullcontext(transaction)), \
             patch.object(saved_queries.product_outbox, "enqueue") as enqueue:
            result = saved_queries.update_saved_query(
                "query-1", {"sql": "SELECT 1"}, "owner@example.test")
        self.assertEqual(result["sql"], "SELECT 1")
        self.assertIn("FOR UPDATE", transaction.calls[0][1])
        self.assertIn("RETURNING id", transaction.calls[1][1])
        self.assertEqual(enqueue.call_args.kwargs["operation"], "update")

    def test_update_missing_row_has_no_event(self):
        transaction = Transaction([])
        with patch.object(saved_queries.db, "transaction",
                          return_value=contextlib.nullcontext(transaction)), \
             patch.object(saved_queries.product_outbox, "enqueue") as enqueue:
            self.assertIsNone(saved_queries.update_saved_query(
                "missing", {"name": "x"}, "owner@example.test"))
        enqueue.assert_not_called()

    def test_delete_locks_and_emits_tombstone(self):
        transaction = Transaction([{"id": "query-1", "created_by": "owner@example.test"}])
        with patch.object(saved_queries.db, "transaction",
                          return_value=contextlib.nullcontext(transaction)), \
             patch.object(saved_queries.product_outbox, "enqueue") as enqueue:
            self.assertTrue(saved_queries.delete_saved_query("query-1", "owner@example.test"))
        self.assertIn("FOR UPDATE", transaction.calls[0][1])
        self.assertEqual(enqueue.call_args.kwargs["payload"], {"id": "query-1", "deleted": True})

    def test_outbox_is_default_off(self):
        transaction=Transaction([source_row()])
        with patch.dict(os.environ,{},clear=True),patch.object(saved_queries.db,"transaction",return_value=contextlib.nullcontext(transaction)),patch.object(saved_queries.product_outbox,"enqueue") as enqueue:
            saved_queries.create_saved_query({"name":"Trips","sql":"SELECT 1"},"owner@example.test")
        enqueue.assert_not_called()


if __name__ == "__main__":
    unittest.main()
