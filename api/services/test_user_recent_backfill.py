import contextlib,sys,tempfile,unittest
from pathlib import Path
from datetime import datetime
from types import SimpleNamespace
from unittest.mock import patch
if "pyodbc" not in sys.modules:sys.modules["pyodbc"]=SimpleNamespace(Error=Exception)
from services import user_recent_backfill as b
from services import user_recent_backfill_operation as operation, user_recents, product_shadow_read
class Tx:
 def __init__(self,rows):self.rows=rows
 def execute(self,*a):pass
 def query_one(self,*a):return {"watermark":7}
 def query(self,*a):return {"rows":self.rows}
@contextlib.contextmanager
def transaction(rows):yield Tx(rows)
def row(item="1",owner="a"):
 return {"user_email":owner,"item_id":item,"label":"Item","href":"/items/"+item,"type":"dashboard","created_at":datetime(2026,1,1)}
class Tests(unittest.TestCase):
 def test_snapshot_is_deterministic_owner_scoped_and_bounded(self):
  with patch.object(b.db,"transaction",return_value=transaction([row("2"),row("1")])):s=b.capture_snapshot()
  self.assertEqual(len(s.records),2);self.assertTrue(all(r.owner_principal=="a" for r in s.records));b.validate(s)
  with patch.object(b.db,"transaction",return_value=transaction([row(str(i)) for i in range(21)])):
   with self.assertRaisesRegex(RuntimeError,"retention"):b.capture_snapshot()
 def test_invalid_type_fails_closed(self):
  bad={**row(),"type":"unknown"}
  with patch.object(b.db,"transaction",return_value=transaction([bad])):
   with self.assertRaisesRegex(RuntimeError,"invalid"):b.capture_snapshot()
 def test_exact_reconciliation(self):
  with patch.object(b.db,"transaction",return_value=transaction([row()])):s=b.capture_snapshot()
  target={"document":s.records[0].document}
  with patch.object(b.product_store,"read",side_effect=[None,target]),patch.object(b.product_store,"transact") as transact:
   self.assertEqual(b.apply_and_reconcile(s)["created"],1)
  transact.assert_called_once()
 def test_checkpoint_resume_and_apply_gate(self):
  with patch.object(b.db,"transaction",return_value=transaction([row()])):snapshot=b.capture_snapshot()
  with tempfile.TemporaryDirectory() as directory:
   path=Path(directory)/"recent.json"
   with patch.object(operation.backfill,"capture_snapshot",return_value=snapshot):self.assertEqual(operation.run(path,apply=False,resume=False)["next_index"],0)
   path.write_text(path.read_text().replace('"next_index":0','"next_index":1'))
   with self.assertRaisesRegex(RuntimeError,"identity"):operation.load(path)
   path.unlink()
   with patch.object(operation.backfill,"capture_snapshot",return_value=snapshot):operation.run(path,apply=False,resume=False)
   with self.assertRaisesRegex(RuntimeError,"KAVEON_USER_RECENT"):operation.run(path,apply=True,resume=True)
   with patch.dict("os.environ",{"KAVEON_USER_RECENT_MIGRATION_ENABLED":"true"}),patch.object(operation.backfill,"apply_and_reconcile",return_value={"family":"user_recents"}):report=operation.run(path,apply=True,resume=True)
   self.assertTrue(report["checkpoint_complete"]);self.assertTrue(operation.load(path)[2])
 def test_shadow_is_default_off_bounded_and_owner_scoped(self):
  self.assertFalse(product_shadow_read.observe_user_recent_list([],"a")["enabled"])
  source=row();target={"document":{**source,"created_at":source["created_at"].isoformat()}}
  with patch.dict("os.environ",{"KAVEON_USER_RECENT_SHADOW_READ_ENABLED":"true"}),patch.object(product_shadow_read.product_store,"read",return_value=target) as read:
   self.assertEqual(product_shadow_read.observe_user_recent_list([source],"a")["status"],"match")
  self.assertEqual(read.call_args.args[2],"a")

class WriterTx:
 def __init__(self,ones=(),rows=()):self.ones=list(ones);self.rows=list(rows);self.calls=[]
 def execute(self,sql,params=None):self.calls.append(("execute",sql,params));return 1
 def query_one(self,sql,params=None):self.calls.append(("one",sql,params));return self.ones.pop(0) if self.ones else None
 def query(self,sql,params=None):self.calls.append(("many",sql,params));return {"rows":self.rows.pop(0) if self.rows else []}
class WriterTests(unittest.TestCase):
 def test_add_serializes_owner_and_emits_create_plus_eviction(self):
  inserted={**row("new"),"id":22};evicted={**row("old"),"id":1};tx=WriterTx([None,inserted],[[evicted]])
  with patch.dict("os.environ",{"METADATA_DB_TYPE":"postgresql"}),patch.object(user_recents.db,"transaction",return_value=contextlib.nullcontext(tx)),patch.object(user_recents.product_outbox,"enqueue") as enqueue:
   user_recents.add_recent("a","new","Item","/items/new","dashboard")
  self.assertIn("pg_advisory_xact_lock",tx.calls[0][1]);self.assertEqual(enqueue.call_count,2)
  self.assertEqual([c.kwargs["operation"] for c in enqueue.call_args_list],["create","delete"])
 def test_cross_owner_delete_fails_before_mutation_above_bound(self):
  rows=[{"id":i,"user_email":str(i),"item_id":"x"} for i in range(user_recents.MAX_CROSS_OWNER_DELETE+1)];tx=WriterTx(rows=[rows])
  with patch.object(user_recents.db,"transaction",return_value=contextlib.nullcontext(tx)),patch.object(user_recents.product_outbox,"enqueue") as enqueue:
   with self.assertRaisesRegex(RuntimeError,"fanout"):user_recents.remove_recent_all_users("x","dashboard")
  enqueue.assert_not_called();self.assertFalse(any(c[0]=="execute" for c in tx.calls))
if __name__=="__main__":unittest.main()
