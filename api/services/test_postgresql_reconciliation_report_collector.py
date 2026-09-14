import hashlib
import json
import os
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from services import postgresql_evidence_collector as evidence_collector
from services import postgresql_reconciliation_report_collector as collector
from services import postgresql_retirement_gate as gate


NOW = __import__("datetime").datetime(2026, 9, 14, 18, tzinfo=__import__("datetime").timezone.utc)


class Record:
    def __init__(self, record_id, document, owner="owner@example.test", kind=None):
        self.record_id, self.document, self.owner_principal = record_id, document, owner
        if kind:
            self.kind = kind


def family_report(family):
    value = {
        "schema_version": 1, "family": family,
        "tables": list(gate.AUTHORITY_FAMILIES[family]), "status": "passed",
        "reconciled_at": "2026-09-14T17:55:00Z", "source_watermark": 7,
        "source_count": 0, "target_count": 0,
        "checks": {name: True for name in gate.REQUIRED_CHECKS},
        "provenance": {"producer": "special-v1", "source_snapshot": "pg:special",
                       "target_snapshot": "kdb:special"},
    }
    value["report_sha256"] = hashlib.sha256(evidence_collector._canonical(value)).hexdigest()
    return value


def inventory(ai_count=0):
    tables = []
    for family, names in gate.AUTHORITY_FAMILIES.items():
        for name in names:
            tables.append({"table": name, "family": family, "present": name in {"ai_providers", "user_ai_keys"},
                           "row_count": ai_count if name == "ai_providers" else 0})
    value = {"schema_version": 1, "captured_at": "2026-09-14T17:50:00Z",
             "source_snapshot": "31:31:", "discovered_tables": [],
             "authority_tables": tables, "infrastructure_tables": [], "unclassified_tables": []}
    value["report_sha256"] = hashlib.sha256(collector._canonical(value)).hexdigest()
    return value


class CollectorTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(); self.root = Path(self.temp.name)
        self.records = {
            "datasets": [Record("ds-1", {"id": "ds-1", "dimensions": [{"id": 1}],
                                           "columns": [{"id": 2}], "metrics": []})],
            "charts": [Record("chart-1", {"id": "chart-1"})],
            "dashboards": [Record("dash-1", {"id": "dash-1"})],
            "favorites": [Record("fav-1", {"id": "fav-1"})],
            "saved_queries": [Record("query-1", {"id": "query-1"})],
            "user_themes": [Record("theme-1", {"id": "theme-1"})],
            "user_recents": [Record("recent-1", {"id": "recent-1"})],
            "query_history": [Record("history-1", {"id": "history-1"})],
            "activity": [Record("activity-1", {"id": "activity-1"})],
            "sources": [Record("catalog-1", {"source_kind": "catalog"}),
                        Record("data-1", {"source_kind": "data"})],
            "chat_history": [Record("session-1", {"id": "session-1"}, kind="chat_session"),
                             Record("message-1", {"id": "message-1"}, kind="chat_message")],
            "dlm_definitions": [Record("ds-1", {"dataset_id": "ds-1"})],
            "dlm_runs": [Record("run-1", {"definition_id": "ds-1"})],
        }
        self.snapshots = {name: SimpleNamespace(records=tuple(records), source_watermark=9,
                                                 snapshot_sha256=(name[0] * 64))
                          for name, records in self.records.items()}
        self.extra_chart = False
        for name in collector.CHECKPOINTS:
            (self.root / f"{name}.json").write_text("{}")
        (self.root / "inventory.json").write_text(json.dumps(inventory()), encoding="utf-8")
        for family in ("context_cache", "dlm_generation"):
            (self.root / f"{family}.json").write_text(json.dumps(family_report(family)), encoding="utf-8")
        manifest = {"schema_version": 1,
                    "checkpoints": {name: f"{name}.json" for name in collector.CHECKPOINTS},
                    "live_inventory": "inventory.json",
                    "special_reports": {"context_cache": "context_cache.json",
                                        "dlm_generation": "dlm_generation.json"}}
        self.manifest = self.root / "manifest.json"
        self.manifest.write_text(json.dumps(manifest), encoding="utf-8")

    def tearDown(self): self.temp.cleanup()

    def target_pages(self, method, path, token_env, actor, **kwargs):
        kind = path.split("/v1/products/", 1)[1].split("?", 1)[0]
        mapping = {"dataset": self.records["datasets"], "chart": self.records["charts"],
                   "dashboard": self.records["dashboards"], "favorite": self.records["favorites"],
                   "saved_query": self.records["saved_queries"], "user_theme": self.records["user_themes"],
                   "user_recent": self.records["user_recents"], "query_history": self.records["query_history"],
                   "activity": self.records["activity"], "source": self.records["sources"],
                   "chat_session": self.records["chat_history"][:1],
                   "chat_message": self.records["chat_history"][1:],
                   "dlm_definition": self.records["dlm_definitions"], "dlm_run": self.records["dlm_runs"]}
        records = [{"id": r.record_id, "document": r.document} for r in mapping[kind]]
        if kind == "chart" and self.extra_chart:
            records.append({"id": "extra", "document": {"id": "extra"}})
        return {"snapshot_id": "snapshot-42", "records": records, "next_cursor": None}

    def run_collect(self):
        loaders = {name: (lambda path, n=name: (self.snapshots[n], len(self.records[n]), True))
                   for name in collector.CHECKPOINTS}
        patched = {name: (loaders[name], family) for name, (_, family) in collector.CHECKPOINTS.items()}
        with patch.dict(os.environ, {"KAVEON_RECONCILIATION_REPORT_COLLECTION_ENABLED": "true"}), \
                patch.object(collector, "CHECKPOINTS", patched), \
                patch.object(collector.engine_bridge, "_request", side_effect=self.target_pages):
            return collector.collect(self.manifest, self.root / "reports", now=NOW)

    def test_emits_exact_16_family_set_and_embedded_semantics(self):
        result = self.run_collect()
        self.assertEqual(result["family_count"], 16)
        names = {path.stem for path in (self.root / "reports").glob("*.json")}
        self.assertEqual(names, set(gate.AUTHORITY_FAMILIES))
        semantics = json.loads((self.root / "reports/dataset_semantics.json").read_text())
        self.assertEqual((semantics["source_count"], semantics["target_count"]), (2, 2))
        self.assertEqual(semantics["provenance"]["target_snapshot"],
                         json.loads((self.root / "reports/datasets.json").read_text())["provenance"]["target_snapshot"])

    def test_incomplete_checkpoint_stops_before_publication(self):
        loaders = {name: (lambda path, n=name: (self.snapshots[n], len(self.records[n]), n != "charts"))
                   for name in collector.CHECKPOINTS}
        patched = {name: (loaders[name], family) for name, (_, family) in collector.CHECKPOINTS.items()}
        with patch.dict(os.environ, {"KAVEON_RECONCILIATION_REPORT_COLLECTION_ENABLED": "true"}), \
                patch.object(collector, "CHECKPOINTS", patched), self.assertRaisesRegex(RuntimeError, "incomplete"):
            collector.collect(self.manifest, self.root / "reports", now=NOW)
        self.assertFalse((self.root / "reports").exists())

    def test_nonzero_ai_authority_is_never_reported_as_retired(self):
        (self.root / "inventory.json").write_text(json.dumps(inventory(1)), encoding="utf-8")
        with self.assertRaisesRegex(RuntimeError, "still contains"):
            self.run_collect()
        self.assertFalse((self.root / "reports").exists())

    def test_extra_target_and_tampered_special_report_fail_closed(self):
        self.extra_chart = True
        with self.assertRaisesRegex(RuntimeError, "extra"):
            self.run_collect()
        self.extra_chart = False
        value = family_report("context_cache"); value["target_count"] = 1
        (self.root / "context_cache.json").write_text(json.dumps(value), encoding="utf-8")
        with self.assertRaisesRegex(RuntimeError, "digest mismatch"):
            self.run_collect()


if __name__ == "__main__": unittest.main()
