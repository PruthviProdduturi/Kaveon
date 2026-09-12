import contextlib,sys,unittest
import tempfile
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch
if "pyodbc" not in sys.modules:sys.modules["pyodbc"]=SimpleNamespace(Error=Exception)
from services import source_backfill as b
from services import source_backfill_operation as operation
class Tx:
 def execute(self,*a):pass
 def query_one(self,*a):return {"watermark":3}
 def query(self,sql,params):
  if "catalog_sources" in sql:return {"rows":[{"id":"1","name":"Lake","engine_catalog":"lake","storage_type":"adls_gen2","data_format":"delta","credential_kind":"managed_identity","credential_ref":None,"adapter_type":"native","lifecycle":"active","description":None,"created_by":"owner"}]}
  return {"rows":[{"id":2,"name":"Warehouse","type":"PostgreSQL","database_name":"warehouse","region":"WW","description":None,"created_by":"owner","is_active":True}]}
@contextlib.contextmanager
def transaction():yield Tx()
class SourceMigrationTests(unittest.TestCase):
 def test_capture_contains_only_public_metadata_and_opaque_secret_refs(self):
  with patch.object(b.db,"transaction",return_value=transaction()):s=b.capture_snapshot()
  self.assertEqual([r.record_id for r in s.records],["catalog-1","data-2"])
  encoded=str([r.document for r in s.records]).lower()
  self.assertNotIn("connection_string",encoded);self.assertNotIn("cipher",encoded)
  self.assertEqual(s.records[1].document["secret_ref"],"key-managed:data_sources/2")
 def test_secret_shaped_fields_and_ambiguous_identity_fail_closed(self):
  s=b.capture_snapshot if False else None
  with self.assertRaisesRegex(RuntimeError,"forbidden"):
   b._safe({"connection_string":"value"})
  doc={"source_kind":"data","source_id":"data:1","catalog_identity":"same","secret_ref":"ref"}
  r1=b.SourceRecord("data:1","o",doc,b._canonical(doc)[1]);doc2={**doc,"source_id":"data:2"};r2=b.SourceRecord("data:2","o",doc2,b._canonical(doc2)[1]);doc3={**doc,"source_id":"data:3"};r3=b.SourceRecord("data:3","o",doc3,b._canonical(doc3)[1])
  snap=b.SourceSnapshot(1,(r1,r2,r3),b.digest((r1,r2,r3)))
  with self.assertRaisesRegex(RuntimeError,"ambiguous"):b.validate_snapshot(snap)
  with self.assertRaisesRegex(RuntimeError,"Key Vault"):
   b._catalog_secret_ref({"credential_ref":"plaintext-secret"})
 def test_exact_owner_reconciliation(self):
  with patch.object(b.db,"transaction",return_value=transaction()):s=b.capture_snapshot()
  targets=[None,None,*({"document":r.document} for r in s.records)]
  with patch.object(b.product_store,"read",side_effect=targets),patch.object(b.product_store,"transact") as tx:
   self.assertEqual(b.apply_and_reconcile(s)["created"],2)
  self.assertEqual(tx.call_count,2)
 def test_checkpoint_resume_is_bounded_and_enabled_explicitly(self):
  with patch.object(b.db,"transaction",return_value=transaction()):snapshot=b.capture_snapshot()
  with tempfile.TemporaryDirectory() as directory:
   checkpoint=Path(directory)/"sources.json"
   with patch.object(operation.backfill,"capture_snapshot",return_value=snapshot):
    report=operation.run(checkpoint,apply=False,resume=False)
   self.assertEqual(report["next_index"],0)
   with self.assertRaisesRegex(RuntimeError,"KAVEON_SOURCE_MIGRATION_ENABLED"):
    operation.run(checkpoint,apply=True,resume=True)
   applied=[]
   with patch.dict("os.environ",{"KAVEON_SOURCE_MIGRATION_ENABLED":"true"}),patch.object(operation.backfill,"apply_and_reconcile",side_effect=lambda value: applied.append(value) or {"family":"sources"}):
    report=operation.run(checkpoint,apply=True,resume=True)
   self.assertTrue(report["checkpoint_complete"]);self.assertEqual(len(applied),len(snapshot.records)+1)
   _,position,complete=operation.load(checkpoint)
   self.assertEqual(position,len(snapshot.records));self.assertTrue(complete)
if __name__=="__main__":unittest.main()
