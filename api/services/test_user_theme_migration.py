import contextlib, json, os, sys, tempfile, unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch
from fastapi import HTTPException
if "pyodbc" not in sys.modules: sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)
from services import theme, user_theme_backfill as backfill, user_theme_backfill_operation as operation

class Tx:
    def __init__(self,current=None,rows=None): self.current,self.rows,self.executed=current,rows or [],[]
    def query_one(self,sql,params=None):
        self.executed.append(("query_one",sql,params)); return self.current if "FOR UPDATE" in sql else {"watermark":8}
    def query(self,sql,params=None): self.executed.append(("query",sql,params)); return {"rows":self.rows}
    def execute(self,sql,params=None): self.executed.append(("execute",sql,params)); return 1
@contextlib.contextmanager
def transaction(tx): yield tx

def snapshot():
    document={"user_email":"owner@example.test","theme_color":"#aabbcc"}
    record=backfill.ThemeRecord("owner@example.test",document,backfill._canonical(document)[1])
    return backfill.ThemeSnapshot(8,(record,),backfill.snapshot_digest((record,)))

class UserThemeMigrationTests(unittest.TestCase):
    def setUp(self): theme._cache.clear()

    def test_save_and_delete_are_atomic_with_one_canonical_outbox_event(self):
        for current,expected in ((None,"create"),({"theme_color":"#000000"},"update")):
            tx=Tx(current)
            with self.subTest(expected=expected),patch.object(theme.db,"transaction",return_value=transaction(tx)),\
                 patch.object(theme.product_outbox,"enqueue") as enqueue:
                theme.save_user_theme("owner@example.test","#AaBbCc")
            enqueue.assert_called_once()
            self.assertEqual(enqueue.call_args.kwargs["operation"],expected)
            self.assertEqual(enqueue.call_args.kwargs["payload"],snapshot().records[0].document)
            self.assertEqual(enqueue.call_args.args[0],tx)
        tx=Tx({"theme_color":"#aabbcc"})
        with patch.object(theme.db,"transaction",return_value=transaction(tx)),patch.object(theme.product_outbox,"enqueue") as enqueue:
            theme.delete_user_theme("owner@example.test")
        self.assertEqual(enqueue.call_args.kwargs["operation"],"delete")
        tx=Tx(None)
        with patch.object(theme.db,"transaction",return_value=transaction(tx)),patch.object(theme.product_outbox,"enqueue") as enqueue:
            theme.delete_user_theme("owner@example.test")
        enqueue.assert_not_called()

    def test_outbox_failure_is_not_swallowed_and_shares_transaction(self):
        tx=Tx(None)
        with patch.object(theme.db,"transaction",return_value=transaction(tx)),\
             patch.object(theme.product_outbox,"enqueue",side_effect=RuntimeError("outbox failure")),\
             self.assertRaisesRegex(RuntimeError,"outbox failure"):
            theme.save_user_theme("owner@example.test","#abcdef")
        tx=Tx(None)
        tx.execute=lambda *_: (_ for _ in ()).throw(RuntimeError("source failure"))
        with patch.object(theme.db,"transaction",return_value=transaction(tx)),\
             patch.object(theme.product_outbox,"enqueue") as enqueue,\
             self.assertRaisesRegex(RuntimeError,"source failure"):
            theme.save_user_theme("owner@example.test","#abcdef")
        enqueue.assert_not_called()

    def test_capture_is_repeatable_bounded_canonical_and_validated(self):
        tx=Tx(rows=[{"user_email":"owner@example.test","theme_color":"#AaBbCc"}])
        with patch.object(backfill.db,"transaction",return_value=transaction(tx)): result=backfill.capture_snapshot()
        self.assertEqual(result,snapshot()); self.assertIn("REPEATABLE READ, READ ONLY",tx.executed[0][1])
        bad=backfill.ThemeSnapshot(8,(backfill.ThemeRecord("",{"user_email":"","theme_color":"bad"},"0"*64),),"0"*64)
        with self.assertRaisesRegex(RuntimeError,"invalid"): backfill.validate_snapshot(bad)

    def test_apply_owner_exact_conflict_and_divergence(self):
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

    def test_checkpoint_default_guard_tamper_and_resume(self):
        value=snapshot()
        with tempfile.TemporaryDirectory() as temporary:
            path=Path(temporary)/"checkpoint.json"
            with patch.object(backfill,"capture_snapshot",return_value=value): self.assertEqual(operation.run(path,apply=False,resume=False)["mode"],"dry-run")
            with patch.dict(os.environ,{},clear=True),self.assertRaisesRegex(RuntimeError,"requires"): operation.run(path,apply=True,resume=True)
            raw=json.loads(path.read_text()); raw["next_index"]=1; path.write_text(json.dumps(raw))
            with self.assertRaisesRegex(RuntimeError,"identity"): operation.load(path)
            operation.save(path,value,0); original=operation.save
            with patch.dict(os.environ,{"KAVEON_USER_THEME_MIGRATION_ENABLED":"true"}),patch.object(backfill,"apply_and_reconcile",return_value={}),\
                 patch.object(operation,"save",side_effect=RuntimeError("checkpoint failure")),self.assertRaisesRegex(RuntimeError,"checkpoint failure"):
                operation.run(path,apply=True,resume=True)
            self.assertEqual(operation.load(path)[1],0)
            with patch.dict(os.environ,{"KAVEON_USER_THEME_MIGRATION_ENABLED":"true"}),\
                 patch.object(backfill,"apply_and_reconcile",side_effect=[{}, {"family":"user_themes"}]) as apply,patch.object(operation,"save",wraps=original):
                self.assertTrue(operation.run(path,apply=True,resume=True)["checkpoint_complete"])
            self.assertEqual(apply.call_count,2)

    def test_shadow_is_default_off_owner_scoped_and_content_free(self):
        with patch.dict(os.environ,{},clear=True),patch.object(theme.product_shadow_read.product_store,"read") as read:
            self.assertEqual(theme.product_shadow_read.observe_user_theme({"theme_color":"#aabbcc"},"owner")["status"],"disabled"); read.assert_not_called()
        exact={"user_email":"owner","theme_color":"#aabbcc"}
        with patch.dict(os.environ,{"KAVEON_USER_THEME_SHADOW_READ_ENABLED":"true"}),\
             patch.object(theme.product_shadow_read.product_store,"read",return_value={"document":exact,"generation":2}) as read:
            report=theme.product_shadow_read.observe_user_theme({"theme_color":"#aabbcc"},"owner")
        self.assertEqual(report["status"],"match"); read.assert_called_once_with("user_theme","owner","owner","Viewer")
        self.assertNotIn("owner",str(report))

if __name__=="__main__": unittest.main()
