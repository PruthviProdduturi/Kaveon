import contextlib,json,os,sys,tempfile,unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch
if "pyodbc" not in sys.modules:sys.modules["pyodbc"]=SimpleNamespace(Error=Exception)
from services import favorites, favorite_backfill as backfill, favorite_backfill_operation as operation, product_shadow_read
class Tx:
 def __init__(self,row=None,rows=None):self.row,self.rows,self.calls=row,rows or [],[]
 def query_one(self,sql,params=None):self.calls.append(("q1",sql,params));return self.row if "FOR UPDATE" in sql else {"watermark":5}
 def query(self,sql,params=None):self.calls.append(("q",sql,params));return {"rows":self.rows}
 def execute(self,sql,params=None):self.calls.append(("x",sql,params));return 1
@contextlib.contextmanager
def transaction(tx):yield tx
def snap():
 d={"user_email":"owner","object_type":"dataset","object_id":"7","object_name":"Orders"};rid=favorites._record_id("owner","dataset","7")
 r=backfill.FavoriteRecord(rid,"owner",d,backfill._canonical(d)[1]);return backfill.FavoriteSnapshot(5,"snap",(r,),backfill.snapshot_digest((r,),"snap"))
class FavoriteMigrationTests(unittest.TestCase):
 def test_source_create_delete_are_atomic_and_data_source_is_explicitly_not_migrated(self):
  tx=Tx()
  with patch.object(favorites.db,"transaction",return_value=transaction(tx)),patch.object(favorites.product_outbox,"enqueue") as enqueue:
   favorites.create_favorite({"object_type":"dataset","object_id":"7","object_name":"Orders"},"owner")
  self.assertEqual(enqueue.call_args.kwargs["record_id"],snap().records[0].record_id)
  tx=Tx({"id":"f","object_type":"dataset","object_id":"7"})
  with patch.object(favorites.db,"transaction",return_value=transaction(tx)),patch.object(favorites.product_outbox,"enqueue") as enqueue:
   self.assertTrue(favorites.delete_favorite("owner","dataset","7"))
  self.assertEqual(enqueue.call_args.kwargs["operation"],"delete")
  tx=Tx()
  with patch.object(favorites.db,"transaction",return_value=transaction(tx)),patch.object(favorites.product_outbox,"enqueue") as enqueue:
   favorites.create_favorite({"object_type":"data_source","object_id":"s","object_name":"Source"},"owner")
  enqueue.assert_not_called()
 def test_source_failure_boundaries_propagate_without_later_work(self):
  tx=Tx();tx.execute=lambda *_:(_ for _ in ()).throw(RuntimeError("source failure"))
  with patch.object(favorites.db,"transaction",return_value=transaction(tx)),patch.object(favorites.product_outbox,"enqueue") as enqueue,self.assertRaisesRegex(RuntimeError,"source failure"):
   favorites.create_favorite({"object_type":"dataset","object_id":"7","object_name":"Orders"},"owner")
  enqueue.assert_not_called()
  with patch.object(favorites.db,"transaction",return_value=transaction(Tx())),patch.object(favorites.product_outbox,"enqueue",side_effect=RuntimeError("outbox failure")),self.assertRaisesRegex(RuntimeError,"outbox failure"):
   favorites.create_favorite({"object_type":"dataset","object_id":"7","object_name":"Orders"},"owner")
 def test_capture_binds_targets_and_rejects_data_sources(self):
  row={"id":"f","user_email":"owner","object_type":"dataset","object_id":"7","object_name":"Orders"}
  with patch.object(backfill.db,"transaction",return_value=transaction(Tx(rows=[row]))),patch.object(backfill.product_store,"read",return_value={"snapshot_id":"snap"}) as read:
   self.assertEqual(backfill.capture_snapshot(),snap())
  read.assert_called_once_with("dataset","7","owner","Admin")
  row["object_type"]="data_source"
  with patch.object(backfill.db,"transaction",return_value=transaction(Tx(rows=[row]))),self.assertRaisesRegex(RuntimeError,"no typed destination"):backfill.capture_snapshot()
 def test_apply_checkpoint_resume_and_shadow(self):
  s=snap();exact={"document":s.records[0].document}
  with patch.object(backfill.product_store,"read",side_effect=[None,exact]),patch.object(backfill.product_store,"transact") as tx:self.assertEqual(backfill.apply_and_reconcile(s)["created"],1)
  self.assertEqual(tx.call_args.args[1:],("owner","Admin"))
  with tempfile.TemporaryDirectory() as td:
   path=Path(td)/"c.json"
   with patch.object(backfill,"capture_snapshot",return_value=s):self.assertEqual(operation.run(path,apply=False,resume=False)["mode"],"dry-run")
   with patch.dict(os.environ,{},clear=True),self.assertRaisesRegex(RuntimeError,"requires"):operation.run(path,apply=True,resume=True)
   raw=json.loads(path.read_text());raw["next_index"]=1;path.write_text(json.dumps(raw))
   with self.assertRaisesRegex(RuntimeError,"identity"):operation.load(path)
   operation.save(path,s,0)
   with patch.dict(os.environ,{"KAVEON_FAVORITE_MIGRATION_ENABLED":"true"}),\
        patch.object(backfill,"apply_and_reconcile",side_effect=[{}, {"family":"favorites"}]) as apply:
    self.assertTrue(operation.run(path,apply=True,resume=True)["checkpoint_complete"])
   self.assertEqual(apply.call_count,2);self.assertEqual(operation.load(path)[1],1)
  source=[{"kind":"dataset","id":"7","name":"Orders"}]
  with patch.dict(os.environ,{"KAVEON_FAVORITE_SHADOW_READ_ENABLED":"true"}),patch.object(product_shadow_read.product_store,"read",return_value=exact) as read:
   self.assertEqual(product_shadow_read.observe_favorite_list(source,"owner")["status"],"match")
  self.assertEqual(read.call_args.args[0],"favorite")
if __name__=="__main__":unittest.main()
