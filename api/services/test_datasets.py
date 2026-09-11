import contextlib
import hashlib
import json
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch
from fastapi import HTTPException

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from services import datasets


class DatasetShadowIntegrationTests(unittest.TestCase):
    def test_list_reports_shadow_and_returns_postgresql_items_unchanged(self):
        row = {
            "id": 7, "dataset_name": "Orders", "description": None,
            "fact_table": "orders", "schema_name": "sales", "database_name": "lake",
            "created_at": "created", "modified_at": "updated", "date_column": None,
            "tables_used": None, "created_by": "alice@example.com",
            "modified_by": "alice@example.com", "visibility": "private", "favorite": 1,
        }
        with patch.object(datasets.db, "query", return_value={"rows": [row]}), \
             patch.object(datasets.product_shadow_read, "compare_dataset_list", return_value={"enabled": True, "status": "match"}) as compare:
            result = datasets.list_datasets("alice@example.com", "Viewer")
        compare.assert_called_once_with(result, "alice@example.com", "Viewer")
        self.assertTrue(result[0]["favorite"])

    def test_user_read_reports_shadow_without_changing_postgresql_response(self):
        parent = {
            "id": 7, "dataset_name": "Orders", "description": None,
            "fact_table": "orders", "schema_name": "sales", "database_name": "lake",
            "created_at": "created", "modified_at": "updated", "date_column": None,
            "tables_used": '{"filters":[]}', "created_by": "alice@example.com",
            "modified_by": "alice@example.com", "visibility": "private", "favorite": 1,
        }
        dimension = {"dimension_table": "region", "table_name": "region", "join_condition": "x.[Name]", "fact_key": "RegionKey", "join_key": "id", "dim_name": "region", "display_name": "Region"}
        column = {"table_name": "region", "column_name": "Name", "data_type": "text", "is_dimension": True, "is_metric": False, "semantic_type": None}
        metric = {"name": "Revenue", "expression": "SUM(revenue)", "metric_type": "sum", "format": None}
        with patch.object(datasets.db, "query_one", return_value=parent), \
             patch.object(datasets.db, "query", side_effect=[{"rows": [dimension]}, {"rows": [column]}, {"rows": [metric]}]), \
             patch.object(datasets.product_shadow_read, "compare_dataset", return_value={"enabled": True, "status": "match"}) as compare:
            result = datasets.get_dataset_by_id("7", "alice@example.com", "Viewer")
        shadow_document, actor, role = compare.call_args.args
        self.assertNotIn("favorite", shadow_document)
        self.assertEqual(shadow_document["columns"], [column])
        self.assertTrue(result["favorite"])
        self.assertEqual(result["columns"][0]["fact_key"], "RegionKey")
        self.assertEqual((actor, role), ("alice@example.com", "Viewer"))


class AtomicWriter:
    def __init__(self, fail_at=None, dataset_id=7, modified_at="2026-09-10T10:00:00"):
        self.fail_at = fail_at
        self.dataset_id = dataset_id
        self.modified_at = modified_at
        self.statement_count = 0
        self.statements = []
        self.committed = False

    def _record(self, sql, params):
        self.statement_count += 1
        self.statements.append((" ".join(sql.split()), params))
        if self.statement_count == self.fail_at:
            raise RuntimeError(f"injected statement {self.statement_count}")

    def query_one(self, sql, params=None):
        self._record(sql, params)
        normalized = " ".join(sql.split())
        if normalized.startswith("INSERT INTO datasets"):
            return {"id": self.dataset_id}
        if "FROM datasets" in normalized and "FOR UPDATE" in normalized:
            return {
                "id": self.dataset_id,
                "modified_at": self.modified_at,
                "created_by": "alice@example.com",
            }
        if "FROM datasets WHERE id" in normalized:
            return {
                "id": self.dataset_id, "dataset_name": "Orders", "description": None,
                "fact_table": "orders", "schema_name": "sales", "database_name": "lake",
                "created_at": "2026-09-10T09:00:00", "modified_at": self.modified_at,
                "date_column": None, "tables_used": '{"filters":[{"column":"region"}]}',
                "created_by": "alice@example.com", "modified_by": "alice@example.com",
                "visibility": "internal", "favorite": 0,
            }
        if "COUNT(*) AS count FROM charts" in normalized:
            return {"count": 0}
        if "INSERT INTO product_migration_outbox" in normalized:
            return {
                "source_sequence": 1,
                "family": params[1],
                "operation": params[2],
                "record_id": params[3],
                "payload_sha256": params[5],
                "actor_principal": params[6],
                "owner_principal": params[7],
            }
        return None

    def query(self, sql, params=None):
        self._record(sql, params)
        normalized = " ".join(sql.split())
        source = dataset_input()
        if "FROM dataset_dimensions" in normalized:
            return {"rows": source["dimensions"], "row_count": 1}
        if "FROM dataset_columns" in normalized:
            return {"rows": source["columns"], "row_count": 1}
        if "FROM dataset_metrics" in normalized:
            return {"rows": source["metrics"], "row_count": 1}
        return {"rows": [], "row_count": 0}

    def execute(self, sql, params=None):
        self._record(sql, params)
        return 1


@contextlib.contextmanager
def atomic_transaction(writer):
    try:
        yield writer
    except Exception:
        writer.committed = False
        raise
    else:
        writer.committed = True


def dataset_input():
    return {
        "name": "Orders", "table_name": "orders", "schema_name": "sales",
        "database_name": "lake",
        "dimensions": [{"dimension_table": "sales.customer", "join_condition": "[CustomerKey] = [CustomerKey]"}],
        "columns": [{"table_name": "orders", "column_name": "amount", "data_type": "decimal", "is_metric": True}],
        "metrics": [{"name": "revenue", "expression": "SUM(amount)", "metric_type": "sum"}],
        "filters": [{"column": "region"}],
    }


class DatasetTransactionTests(unittest.TestCase):
    def test_create_commits_parent_children_and_exactly_one_canonical_outbox_event(self):
        writer = AtomicWriter()
        with patch.object(datasets.db, "transaction", return_value=atomic_transaction(writer)), \
             patch.object(datasets, "get_dataset_by_id", return_value={"id": "7"}):
            self.assertEqual(datasets.create_dataset(dataset_input(), "alice@example.com"), {"id": "7"})
        self.assertTrue(writer.committed)
        outbox = [entry for entry in writer.statements if "INSERT INTO product_migration_outbox" in entry[0]]
        self.assertEqual(len(outbox), 1)
        params = outbox[0][1]
        payload = json.loads(params[4])
        self.assertEqual(payload["id"], "7")
        self.assertEqual(payload["dimensions"], dataset_input()["dimensions"])
        self.assertEqual(payload["metrics"], dataset_input()["metrics"])
        self.assertEqual(params[5], hashlib.sha256(params[4].encode("utf-8")).hexdigest())

    def test_create_failure_after_each_statement_has_no_commit_or_partial_read(self):
        # Parent/children, four canonical source reads, then outbox.
        for fail_at in range(1, 10):
            with self.subTest(fail_at=fail_at):
                writer = AtomicWriter(fail_at=fail_at)
                with patch.object(datasets.db, "transaction", return_value=atomic_transaction(writer)), \
                     patch.object(datasets, "get_dataset_by_id") as read:
                    with self.assertRaisesRegex(RuntimeError, "injected"):
                        datasets.create_dataset(dataset_input(), "alice@example.com")
                self.assertFalse(writer.committed)
                read.assert_not_called()
                outbox_count = sum("INSERT INTO product_migration_outbox" in sql for sql, _ in writer.statements)
                self.assertEqual(outbox_count, 1 if fail_at == 9 else 0)

    def test_update_is_atomic_at_every_parent_child_and_outbox_statement(self):
        existing = {
            "id": "7", "name": "Orders", "visibility": "internal",
            "created_by": "alice@example.com",
            "tables_used": '{"filters":[]}', "modified_at": "2026-09-10T10:00:00",
            "dimensions": [], "columns": [], "metrics": [], "filters": [],
        }
        update = {
            "name": "Orders v2", "dimensions": dataset_input()["dimensions"],
            "columns": dataset_input()["columns"], "metrics": dataset_input()["metrics"],
        }
        for fail_at in range(1, 14):
            with self.subTest(fail_at=fail_at):
                writer = AtomicWriter(fail_at=fail_at)
                with patch.object(datasets, "get_dataset_by_id", return_value=existing), \
                     patch.object(datasets.db, "transaction", return_value=atomic_transaction(writer)):
                    with self.assertRaisesRegex(RuntimeError, "injected"):
                        datasets.update_dataset("7", update, "alice@example.com")
                self.assertFalse(writer.committed)

    def test_update_commits_exactly_one_outbox_event_after_all_children(self):
        existing = {
            "id": "7", "name": "Orders", "visibility": "internal",
            "created_by": "alice@example.com",
            "tables_used": "{}", "modified_at": "2026-09-10T10:00:00",
            "dimensions": [], "columns": [], "metrics": [], "filters": [],
        }
        writer = AtomicWriter()
        with patch.object(datasets, "get_dataset_by_id", side_effect=[existing, {"id": "7"}]), \
             patch.object(datasets.db, "transaction", return_value=atomic_transaction(writer)):
            self.assertEqual(
                datasets.update_dataset("7", {"columns": dataset_input()["columns"]}, "alice@example.com"),
                {"id": "7"},
            )
        self.assertTrue(writer.committed)
        outbox_positions = [
            index for index, (sql, _) in enumerate(writer.statements)
            if "INSERT INTO product_migration_outbox" in sql
        ]
        self.assertEqual(outbox_positions, [len(writer.statements) - 1])

    def test_virtual_sql_and_unrelated_metadata_survive_filter_refresh(self):
        existing = {
            "id": "7", "name": "Leaderboard", "visibility": "internal",
            "created_by": "alice@example.com",
            "modified_at": "2026-09-10T10:00:00", "dimensions": [], "columns": [],
            "metrics": [], "filters": [],
            "tables_used": json.dumps({
                "sql_text": "SELECT model, score FROM ai_benchmarks.leaderboard",
                "filters": [{"column": "model"}], "seed_revision": "showcase-v1",
            }),
        }
        writer = AtomicWriter()
        with patch.object(datasets, "get_dataset_by_id", side_effect=[existing, {"id": "7"}]), \
             patch.object(datasets.db, "transaction", return_value=atomic_transaction(writer)):
            result = datasets.update_dataset(
                "7", {"sql_text": "SELECT model, score FROM ai_benchmarks.leaderboard WHERE score IS NOT NULL",
                      "filters": [{"column": "provider"}]}, "seed@example.com",
            )
        self.assertEqual(result, {"id": "7"})
        parent_params = next(params for sql, params in writer.statements if sql.startswith("UPDATE datasets"))
        updated = next(value for value in parent_params if isinstance(value, str) and "sql_text" in value)
        self.assertEqual(json.loads(updated), {
            "sql_text": "SELECT model, score FROM ai_benchmarks.leaderboard WHERE score IS NOT NULL",
            "filters": [{"column": "provider"}], "seed_revision": "showcase-v1",
        })

    def test_concurrent_update_is_rejected_before_write_or_outbox(self):
        existing = {"id": "7", "name": "Orders", "visibility": "internal", "created_by": "alice@example.com", "tables_used": "{}", "modified_at": "old"}
        writer = AtomicWriter(modified_at="new")
        with patch.object(datasets, "get_dataset_by_id", return_value=existing), \
             patch.object(datasets.db, "transaction", return_value=atomic_transaction(writer)):
            with self.assertRaises(HTTPException) as error:
                datasets.update_dataset("7", {"name": "Changed"}, "alice@example.com")
        self.assertEqual(error.exception.status_code, 409)
        self.assertEqual(writer.statement_count, 1)
        self.assertFalse(writer.committed)

    def test_delete_is_atomic_and_emits_one_tombstone(self):
        for fail_at in range(1, 5):
            with self.subTest(fail_at=fail_at):
                writer = AtomicWriter(fail_at=fail_at)
                with patch.object(datasets.db, "transaction", return_value=atomic_transaction(writer)):
                    with self.assertRaisesRegex(RuntimeError, "injected"):
                        datasets.delete_dataset("7", "alice@example.com")
                self.assertFalse(writer.committed)

        writer = AtomicWriter()
        with patch.object(datasets.db, "transaction", return_value=atomic_transaction(writer)):
            self.assertTrue(datasets.delete_dataset("7", "alice@example.com"))
        self.assertTrue(writer.committed)
        outbox = [params for sql, params in writer.statements if "INSERT INTO product_migration_outbox" in sql]
        self.assertEqual(len(outbox), 1)
        self.assertEqual(json.loads(outbox[0][4]), {"deleted": True, "id": "7"})

    def test_delete_rejects_uncaptured_chart_cascade(self):
        writer = AtomicWriter()
        original_query = writer.query_one

        def query_with_chart(sql, params=None):
            row = original_query(sql, params)
            if "COUNT(*) AS count FROM charts" in " ".join(sql.split()):
                return {"count": 2}
            return row

        writer.query_one = query_with_chart
        with patch.object(datasets.db, "transaction", return_value=atomic_transaction(writer)):
            with self.assertRaises(HTTPException) as error:
                datasets.delete_dataset("7", "alice@example.com")
        self.assertEqual(error.exception.status_code, 409)
        self.assertFalse(writer.committed)
        self.assertFalse(any(sql.startswith("DELETE FROM datasets") for sql, _ in writer.statements))


if __name__ == "__main__":
    unittest.main()
