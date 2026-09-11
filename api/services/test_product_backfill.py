import contextlib
import hashlib
import json
import sys
import unittest
from types import SimpleNamespace
from unittest.mock import patch

from fastapi import HTTPException

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from services import product_backfill


class SnapshotTransaction:
    def __init__(self, parents=None):
        self.parents = parents or [{
            "id": 2, "dataset_name": "Orders", "description": None,
            "fact_table": "orders", "schema_name": "sales", "database_name": "lake",
            "created_at": "2026-09-10T09:00:00", "modified_at": "2026-09-10T10:00:00",
            "date_column": None, "tables_used": '{"filters":[{"column":"region"}]}',
            "created_by": "owner@example.com", "modified_by": "editor@example.com",
            "visibility": "internal", "favorite": 0,
        }]
        self.statements = []

    def execute(self, sql, params=None):
        self.statements.append((" ".join(sql.split()), params))
        return 0

    def query_one(self, sql, params=None):
        self.statements.append((" ".join(sql.split()), params))
        return {"watermark": 41}

    def query(self, sql, params=None):
        normalized = " ".join(sql.split())
        self.statements.append((normalized, params))
        if "FROM datasets ORDER BY" in normalized:
            rows = self.parents
        elif "FROM dataset_dimensions" in normalized:
            rows = [{"dataset_id": 2, "dimension_table": "sales.region", "table_name": "region"}]
        elif "FROM dataset_columns" in normalized:
            rows = [{"dataset_id": 2, "table_name": "orders", "column_name": "amount"}]
        elif "FROM dataset_metrics" in normalized:
            rows = [{"dataset_id": 2, "name": "revenue", "expression": "SUM(amount)"}]
        else:
            rows = []
        return {"rows": rows, "row_count": len(rows)}


def snapshot_record():
    document = {"id": "2", "name": "Orders"}
    payload = json.dumps(document, sort_keys=True, separators=(",", ":"))
    return product_backfill.SnapshotRecord(
        "2", "owner@example.com", document, hashlib.sha256(payload.encode()).hexdigest()
    )


def snapshot():
    return product_backfill.DatasetSnapshot(41, (snapshot_record(),), "f" * 64)


class ProductBackfillTests(unittest.TestCase):
    def test_repeatable_snapshot_captures_watermark_and_canonical_children(self):
        transaction = SnapshotTransaction()
        with patch.object(
            product_backfill.db, "transaction", return_value=contextlib.nullcontext(transaction)
        ):
            result = product_backfill.capture_dataset_snapshot()
        self.assertTrue(transaction.statements[0][0].startswith(
            "SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY"
        ))
        self.assertEqual(result.source_watermark, 41)
        self.assertEqual(len(result.records), 1)
        record = result.records[0]
        self.assertEqual(record.document["dimensions"][0]["table_name"], "region")
        self.assertEqual(record.document["filters"], [{"column": "region"}])
        canonical = json.dumps(record.document, sort_keys=True, separators=(",", ":"))
        self.assertEqual(record.payload_sha256, hashlib.sha256(canonical.encode()).hexdigest())

    def test_snapshot_record_bound_fails_before_any_target_write(self):
        parents = [{"id": index} for index in range(product_backfill.MAX_DATASETS + 1)]
        transaction = SnapshotTransaction(parents)
        with patch.object(
            product_backfill.db, "transaction", return_value=contextlib.nullcontext(transaction)
        ), patch.object(product_backfill.product_store, "transact") as target:
            with self.assertRaisesRegex(RuntimeError, "configured bound"):
                product_backfill.capture_dataset_snapshot()
        target.assert_not_called()

    def test_apply_creates_missing_record_then_reconciles_exactly(self):
        record = snapshot_record()
        target = {"document": record.document, "revision": 1, "generation": 7}
        with patch.object(product_backfill.product_store, "read", side_effect=[None, target]), \
             patch.object(product_backfill.product_store, "transact", return_value={"generation": 7}) as transact:
            report = product_backfill.apply_and_reconcile(snapshot())
        self.assertEqual(report["created"], 1)
        self.assertEqual(report["reconciled"], 1)
        self.assertEqual(report["source_watermark"], 41)
        self.assertEqual(transact.call_args.args[1:], ("owner@example.com", "Admin"))

    def test_rerun_skips_exact_record_without_new_generation(self):
        record = snapshot_record()
        target = {"document": record.document, "revision": 1, "generation": 7}
        with patch.object(product_backfill.product_store, "read", return_value=target), \
             patch.object(product_backfill.product_store, "transact") as transact:
            report = product_backfill.apply_and_reconcile(snapshot())
        self.assertEqual((report["created"], report["already_present"]), (0, 1))
        transact.assert_not_called()

    def test_ambiguous_create_resolves_only_from_exact_target(self):
        record = snapshot_record()
        target = {"document": record.document, "revision": 1, "generation": 7}
        with patch.object(product_backfill.product_store, "read", side_effect=[None, target, target]), \
             patch.object(product_backfill.product_store, "transact", side_effect=HTTPException(409, "conflict")):
            self.assertEqual(product_backfill.apply_and_reconcile(snapshot())["reconciled"], 1)

    def test_existing_divergent_record_fails_before_write(self):
        target = {"document": {"id": "2", "name": "Different"}, "revision": 1, "generation": 7}
        with patch.object(product_backfill.product_store, "read", return_value=target), \
             patch.object(product_backfill.product_store, "transact") as transact:
            with self.assertRaisesRegex(RuntimeError, "differs"):
                product_backfill.apply_and_reconcile(snapshot())
        transact.assert_not_called()

    def test_post_create_reconciliation_failure_never_returns_success_report(self):
        divergent = {
            "document": {"id": "2", "name": "Different"},
            "revision": 1,
            "generation": 7,
        }
        with patch.object(product_backfill.product_store, "read", side_effect=[None, divergent]), \
             patch.object(product_backfill.product_store, "transact", return_value={"generation": 7}):
            with self.assertRaisesRegex(RuntimeError, "failed reconciliation"):
                product_backfill.apply_and_reconcile(snapshot())


if __name__ == "__main__":
    unittest.main()
