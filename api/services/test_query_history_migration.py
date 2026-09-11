import contextlib,sys,unittest
from datetime import datetime
from types import SimpleNamespace
from unittest.mock import patch
if "pyodbc" not in sys.modules:sys.modules["pyodbc"]=SimpleNamespace(Error=Exception)
from services import query_history_backfill as b,query_history,product_shadow_read
def row(id="q1",owner="a"):
 return {"id":id,"sql_text":"SELECT 1","database_name":"db","executed_at":datetime(2026,1,1),"execution_time":1,"row_count":1,"status":"success","error_message":None,"user_email":owner,"trigger_source":"lab","dataset_id":None,"tables_used":"[]"}
class Tx:
 def __init__(self,ones=(),rows=()):self.ones=list(ones);self.rows=list(rows);self.calls=[]
 def execute(self,sql,params=None):self.calls.append(("execute",sql,params));return 1
 def query_one(self,sql,params=None):self.calls.append(("one",sql,params));return self.ones.pop(0) if self.ones else {"watermark":3}
 def query(self,sql,params=None):self.calls.append(("many",sql,params));return {"rows":self.rows.pop(0) if self.rows else []}
@contextlib.contextmanager
def transaction(tx):yield tx
class Tests(unittest.TestCase):
 def test_deterministic_snapshot_and_exact_reconciliation(self):
  tx=Tx(rows=[[row("q2"),row("q1")]])
  with patch.object(b.db,"transaction",return_value=transaction(tx)):snapshot=b.capture_snapshot()
  self.assertEqual([r.record_id for r in snapshot.records],["q1","q2"]);b.validate(snapshot)
  targets=[None,None,*({"document":r.document} for r in snapshot.records)]
  with patch.object(b.product_store,"read",side_effect=targets),patch.object(b.product_store,"transact") as transact:self.assertEqual(b.apply_and_reconcile(snapshot)["created"],2)
  self.assertEqual(transact.call_count,2)
 def test_enabled_writer_and_event_share_transaction_and_failure_escapes(self):
  tx=Tx(rows=[[]])
  with patch.dict("os.environ",{"KAVEON_QUERY_HISTORY_OUTBOX_ENABLED":"true"}),patch.object(query_history,"_supports_engine_details",return_value=False),patch.object(query_history.db,"transaction",return_value=transaction(tx)),patch.object(query_history.product_outbox,"enqueue",side_effect=RuntimeError("outbox failed")) as enqueue:
   with self.assertRaisesRegex(RuntimeError,"outbox failed"):query_history.create_history({"sql_text":"SELECT 1","status":"success"},"a")
  self.assertEqual(tx.calls[0][0],"execute");self.assertIs(enqueue.call_args.args[0],tx);self.assertNotIn("engine_details",enqueue.call_args.kwargs["payload"])
 def test_delete_fanout_fails_before_mutation(self):
  rows=[{"id":str(i)} for i in range(query_history.MAX_DELETE_FANOUT+1)];tx=Tx(rows=[rows])
  with patch.dict("os.environ",{"KAVEON_QUERY_HISTORY_OUTBOX_ENABLED":"true"}),patch.object(query_history.db,"transaction",return_value=transaction(tx)),patch.object(query_history.product_outbox,"enqueue") as enqueue:
   with self.assertRaisesRegex(RuntimeError,"fanout"):query_history.delete_all_history("a")
  enqueue.assert_not_called();self.assertFalse(any(c[0]=="execute" for c in tx.calls))
 def test_shadow_default_off_and_owner_scoped(self):
  source=row();self.assertFalse(product_shadow_read.observe_query_history_list([source],"a")["enabled"])
  with patch.dict("os.environ",{"KAVEON_QUERY_HISTORY_SHADOW_READ_ENABLED":"true"}),patch.object(product_shadow_read.product_store,"read",return_value={"document":b.document(source)}) as read:
   self.assertEqual(product_shadow_read.observe_query_history_list([source],"a")["status"],"match")
  self.assertEqual(read.call_args.args[2],"a")
if __name__=="__main__":unittest.main()
