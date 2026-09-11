import contextlib,sys,tempfile,unittest
from pathlib import Path
from datetime import datetime
from types import SimpleNamespace
from unittest.mock import patch
if "pyodbc" not in sys.modules:sys.modules["pyodbc"]=SimpleNamespace(Error=Exception)
from services import activity_backfill as b,activity_backfill_operation as operation,product_shadow_read
from routers import catalog_sources
def row(id="a1"):
 return {"id":id,"action":"created","object_type":"catalog_source","object_id":"c1","object_name":"Lake","timestamp":datetime(2026,1,1),"user_email":"owner","details":'{"storage_type":"adls_gen2"}'}
class Tx:
 def __init__(self,ones=(),rows=()):self.ones=list(ones);self.rows=list(rows);self.calls=[]
 def execute(self,sql,params=None):self.calls.append(("execute",sql,params));return 1
 def query_one(self,sql,params=None):self.calls.append(("one",sql,params));return self.ones.pop(0) if self.ones else {"watermark":4}
 def query(self,sql,params=None):self.calls.append(("many",sql,params));return {"rows":self.rows.pop(0) if self.rows else []}
@contextlib.contextmanager
def transaction(tx):yield tx
class Tests(unittest.TestCase):
 def test_snapshot_rejects_secret_shaped_details(self):
  tx=Tx(rows=[[row()]])
  with patch.object(b.db,"transaction",return_value=transaction(tx)):snapshot=b.capture_snapshot()
  b.validate(snapshot);self.assertEqual(snapshot.records[0].document["details"]["storage_type"],"adls_gen2")
  with self.assertRaisesRegex(RuntimeError,"forbidden"):b.document({**row(),"details":'{"password":"x"}'})
 def test_exact_reconciliation(self):
  tx=Tx(rows=[[row()]])
  with patch.object(b.db,"transaction",return_value=transaction(tx)):snapshot=b.capture_snapshot()
  with patch.object(b.product_store,"read",side_effect=[None,{"document":snapshot.records[0].document}]),patch.object(b.product_store,"transact") as transact:self.assertEqual(b.apply_and_reconcile(snapshot)["created"],1)
  transact.assert_called_once()
 def test_writer_default_off_and_enabled_failure_uses_same_transaction(self):
  tx=Tx(ones=[row()])
  with patch.dict("os.environ",{},clear=True),patch.object(catalog_sources.db,"execute") as execute,patch.object(catalog_sources.product_outbox,"enqueue") as enqueue:catalog_sources._audit("created","c1","Lake","owner")
  execute.assert_called_once();enqueue.assert_not_called()
  with patch.dict("os.environ",{"KAVEON_ACTIVITY_OUTBOX_ENABLED":"true"}),patch.object(catalog_sources.db,"transaction",return_value=transaction(tx)),patch.object(catalog_sources.product_outbox,"enqueue",side_effect=RuntimeError("outbox failed")) as enqueue:
   with self.assertRaisesRegex(RuntimeError,"outbox failed"):catalog_sources._audit("created","c1","Lake","owner")
  self.assertIs(enqueue.call_args.args[0],tx)
 def test_shadow_is_default_off_bounded_and_actor_isolated(self):
  source=row();self.assertFalse(product_shadow_read.observe_activity_list([source])["enabled"])
  with patch.dict("os.environ",{"KAVEON_ACTIVITY_SHADOW_READ_ENABLED":"true"}),patch.object(product_shadow_read.product_store,"read",return_value={"document":b.document(source)}) as read:self.assertEqual(product_shadow_read.observe_activity_list([source])["status"],"match")
  self.assertEqual(read.call_args.args[2],"owner")
 def test_checkpoint_is_tamper_evident_resumable_and_default_off(self):
  tx=Tx(rows=[[row()]])
  with patch.object(b.db,"transaction",return_value=transaction(tx)):snapshot=b.capture_snapshot()
  with tempfile.TemporaryDirectory() as directory:
   path=Path(directory)/"activity.json"
   with patch.object(operation.backfill,"capture_snapshot",return_value=snapshot):operation.run(path,apply=False,resume=False)
   original=path.read_text();path.write_text(original.replace('"next_index":0','"next_index":1'))
   with self.assertRaisesRegex(RuntimeError,"identity"):operation.load(path)
   path.write_text(original)
   with self.assertRaisesRegex(RuntimeError,"KAVEON_ACTIVITY_MIGRATION_ENABLED"):operation.run(path,apply=True,resume=True)
   with patch.dict("os.environ",{"KAVEON_ACTIVITY_MIGRATION_ENABLED":"true"}),patch.object(operation.backfill,"apply_and_reconcile",return_value={"family":"activity"}):report=operation.run(path,apply=True,resume=True)
   self.assertTrue(report["checkpoint_complete"])
if __name__=="__main__":unittest.main()
