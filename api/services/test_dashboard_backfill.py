import contextlib, hashlib, json, os, sys, tempfile, unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch
from fastapi import HTTPException
if "pyodbc" not in sys.modules: sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)
from services import dashboard_backfill as backfill
from services import dashboard_backfill_operation as operation

class Source:
    def __init__(self, rows): self.rows, self.statements = rows, []
    def execute(self, sql, params=None): self.statements.append(" ".join(sql.split()))
    def query_one(self, sql, params=None): return {"watermark": 9}
    def query(self, sql, params=None): self.statements.append(" ".join(sql.split())); return {"rows": self.rows}
@contextlib.contextmanager
def transaction(source): yield source

def row(): return {"id":"d-1","name":"Ops","description":None,"layout":"[]","charts":"[\"c-1\"]",
    "filters":"[]","theme":None,"visibility":"private","is_published":False,"is_archived":False,
    "created_by":"owner@example.test","modified_by":"owner@example.test","created_at":"2026-01-01",
    "modified_at":"2026-01-02"}
def snapshot():
    document={"id":"d-1","name":"Ops","description":None,"layout":[],"charts":["c-1"],
      "chart_revisions":{"c-1":2},"filters":[],"theme":None,"visibility":"private",
      "is_published":False,"is_archived":False,"created_by":"owner@example.test",
      "modified_by":"owner@example.test","created_at":"2026-01-01","updated_at":"2026-01-02"}
    record=backfill.DashboardRecord("d-1","owner@example.test",document,backfill._canonical(document)[1])
    return backfill.DashboardSnapshot(9,"snap-1",(record,),backfill.snapshot_digest((record,),"snap-1"))

class DashboardBackfillTests(unittest.TestCase):
    def test_capture_is_repeatable_owner_scoped_and_chart_revision_bound(self):
        source=Source([row()])
        with patch.object(backfill.db,"transaction",return_value=transaction(source)), \
             patch.object(backfill.product_store,"read",return_value={"revision":2,"snapshot_id":"snap-1"}) as read:
            result=backfill.capture_snapshot()
        self.assertIn("REPEATABLE READ, READ ONLY",source.statements[0]); self.assertEqual(result,snapshot())
        self.assertEqual(read.call_args.args,("chart","c-1","owner@example.test","Admin"))

    def test_capture_rejects_malformed_duplicate_missing_and_mixed_references(self):
        bad=row(); bad["charts"]="bad"
        duplicate=row(); duplicate["charts"]='["c-1","c-1"]'
        for source_rows, targets, message in (([bad],[],"charts is invalid"),([duplicate],[],"references"),
                ([row()],[None],"missing for dashboard")):
            with self.subTest(message=message),patch.object(backfill.db,"transaction",return_value=transaction(Source(source_rows))),\
                 patch.object(backfill.product_store,"read",side_effect=targets),self.assertRaisesRegex(RuntimeError,message):
                backfill.capture_snapshot()
        second=row(); second["id"]="d-2"; second["charts"]='["c-2"]'
        with patch.object(backfill.db,"transaction",return_value=transaction(Source([row(),second]))),\
             patch.object(backfill.product_store,"read",side_effect=[{"revision":1,"snapshot_id":"a"},{"revision":1,"snapshot_id":"b"}]),\
             self.assertRaisesRegex(RuntimeError,"snapshot"):
            backfill.capture_snapshot()

    def test_apply_exact_owner_conflict_and_divergence(self):
        value=snapshot(); exact={"document":value.records[0].document}
        with patch.object(backfill.product_store,"read",side_effect=[None,exact]),patch.object(backfill.product_store,"transact") as tx:
            self.assertEqual(backfill.apply_and_reconcile(value)["created"],1)
        self.assertEqual(tx.call_args.args[1:],("owner@example.test","Admin"))
        with patch.object(backfill.product_store,"read",side_effect=[None,exact,exact]),\
             patch.object(backfill.product_store,"transact",side_effect=HTTPException(409,"conflict")):
            self.assertEqual(backfill.apply_and_reconcile(value)["reconciled"],1)
        with patch.object(backfill.product_store,"read",return_value={"document":{}}),patch.object(backfill.product_store,"transact") as tx:
            with self.assertRaisesRegex(RuntimeError,"diverges"): backfill.apply_and_reconcile(value)
        tx.assert_not_called()

    def test_checkpoint_guard_tamper_and_post_commit_resume(self):
        value=snapshot()
        with tempfile.TemporaryDirectory() as temporary:
            path=Path(temporary)/"checkpoint.json"
            with patch.object(backfill,"capture_snapshot",return_value=value): self.assertEqual(operation.run(path,apply=False,resume=False)["mode"],"dry-run")
            with patch.dict(os.environ,{},clear=True),self.assertRaisesRegex(RuntimeError,"requires"): operation.run(path,apply=True,resume=True)
            raw=json.loads(path.read_text()); raw["next_index"]=1; path.write_text(json.dumps(raw))
            with self.assertRaisesRegex(RuntimeError,"identity"): operation.load(path)
            operation.save(path,value,0); original=operation.save
            with patch.dict(os.environ,{"KAVEON_DASHBOARD_MIGRATION_ENABLED":"true"}),patch.object(backfill,"apply_and_reconcile",return_value={}),\
                 patch.object(operation,"save",side_effect=RuntimeError("checkpoint failure")),self.assertRaisesRegex(RuntimeError,"checkpoint failure"):
                operation.run(path,apply=True,resume=True)
            self.assertEqual(operation.load(path)[1],0)
            with patch.dict(os.environ,{"KAVEON_DASHBOARD_MIGRATION_ENABLED":"true"}),\
                 patch.object(backfill,"apply_and_reconcile",side_effect=[{}, {"family":"dashboards"}]) as apply,\
                 patch.object(operation,"save",wraps=original): report=operation.run(path,apply=True,resume=True)
            self.assertEqual(apply.call_count,2); self.assertTrue(report["checkpoint_complete"])

if __name__ == "__main__": unittest.main()
