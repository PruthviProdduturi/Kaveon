import contextlib
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from fastapi import HTTPException

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from services import user_theme_backfill as backfill
from services import user_theme_backfill_operation as operation


class Source:
    def __init__(self, rows):
        self.rows = rows
        self.statements = []

    def execute(self, sql, params=None):
        self.statements.append(" ".join(sql.split()))

    def query_one(self, sql, params=None):
        return {"watermark": 23}

    def query(self, sql, params=None):
        self.statements.append(" ".join(sql.split()))
        return {"rows": self.rows}


@contextlib.contextmanager
def transaction(source):
    yield source


def row(owner="owner@example.test", color="#abcdef"):
    return {"user_email": owner, "theme_color": color}


def snapshot():
    document = {"user_email": "owner@example.test", "theme_color": "#abcdef"}
    record = backfill.ThemeRecord(
        "owner@example.test",
        document,
        backfill._canonical(document)[1],
    )
    return backfill.ThemeSnapshot(
        23,
        (record,),
        backfill.snapshot_digest((record,)),
    )


class UserThemeBackfillTests(unittest.TestCase):
    def test_capture_is_repeatable_read_and_normalizes_theme_color(self):
        source = Source([row(color="#ABCDEF")])
        with patch.object(backfill.db, "transaction", return_value=transaction(source)):
            captured = backfill.capture_snapshot()
        self.assertEqual(captured, snapshot())
        self.assertIn("REPEATABLE READ, READ ONLY", source.statements[0])

    def test_validation_rejects_bad_owner_color_order_and_digest(self):
        value = snapshot()
        bad_owner = backfill.ThemeRecord(
            "other@example.test",
            value.records[0].document,
            value.records[0].payload_sha256,
        )
        with self.assertRaisesRegex(RuntimeError, "user theme .* invalid"):
            backfill.validate_snapshot(
                backfill.ThemeSnapshot(23, (bad_owner,), backfill.snapshot_digest((bad_owner,)))
            )

        bad_document = {"user_email": "owner@example.test", "theme_color": "blue"}
        bad_color = backfill.ThemeRecord(
            "owner@example.test", bad_document, backfill._canonical(bad_document)[1]
        )
        with self.assertRaisesRegex(RuntimeError, "user theme .* invalid"):
            backfill.validate_snapshot(
                backfill.ThemeSnapshot(23, (bad_color,), backfill.snapshot_digest((bad_color,)))
            )

        reversed_records = tuple(
            backfill.ThemeRecord(
                owner,
                {"user_email": owner, "theme_color": "#abcdef"},
                backfill._canonical({"user_email": owner, "theme_color": "#abcdef"})[1],
            )
            for owner in ("z@example.test", "a@example.test")
        )
        with self.assertRaisesRegex(RuntimeError, "order"):
            backfill.validate_snapshot(
                backfill.ThemeSnapshot(23, reversed_records, backfill.snapshot_digest(reversed_records))
            )

    def test_apply_reconciles_exact_document_and_rejects_divergence(self):
        value = snapshot()
        exact = {"document": value.records[0].document}
        with patch.object(backfill.product_store, "read", side_effect=[None, exact]), \
                patch.object(backfill.product_store, "transact") as transact:
            report = backfill.apply_and_reconcile(value)
        self.assertEqual(report["created"], 1)
        self.assertEqual(report["reconciled"], 1)
        transact.assert_called_once()

        with patch.object(backfill.product_store, "read", return_value={"document": {}}), \
                patch.object(backfill.product_store, "transact") as transact:
            with self.assertRaisesRegex(RuntimeError, "diverges"):
                backfill.apply_and_reconcile(value)
        transact.assert_not_called()

        with patch.object(backfill.product_store, "read", side_effect=[None, exact, exact]), \
                patch.object(
                    backfill.product_store,
                    "transact",
                    side_effect=HTTPException(409, "conflict"),
                ):
            report = backfill.apply_and_reconcile(value)
        self.assertEqual(report["reconciled"], 1)

    def test_checkpoint_is_tamper_evident_and_resume_is_gated(self):
        value = snapshot()
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "themes.json"
            with patch.object(backfill, "capture_snapshot", return_value=value):
                dry_run = operation.run(path, apply=False, resume=False)
            self.assertEqual(dry_run["snapshot_sha256"], value.snapshot_sha256)

            raw = json.loads(path.read_text())
            raw["next_index"] = 1
            path.write_text(json.dumps(raw))
            with self.assertRaisesRegex(RuntimeError, "identity"):
                operation.load(path)

            operation.save(path, value, 0)
            with self.assertRaisesRegex(RuntimeError, "KAVEON_USER_THEME"):
                operation.run(path, apply=True, resume=True)

            final = {"family": "user_themes", "source_count": 1, "reconciled": 1}
            with patch.dict(os.environ, {"KAVEON_USER_THEME_MIGRATION_ENABLED": "true"}), \
                    patch.object(backfill, "apply_and_reconcile", side_effect=[{}, final]) as apply:
                result = operation.run(path, apply=True, resume=True)
            self.assertTrue(result["checkpoint_complete"])
            self.assertEqual(result["reconciled"], 1)
            self.assertEqual(apply.call_count, 2)
            self.assertEqual(operation.load(path)[1:], (1, True))


if __name__ == "__main__":
    unittest.main()
