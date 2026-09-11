import contextlib,sys,tempfile,unittest
from pathlib import Path
from datetime import datetime
from types import SimpleNamespace
from unittest.mock import patch
if "pyodbc" not in sys.modules:sys.modules["pyodbc"]=SimpleNamespace(Error=Exception)
from services import chat_history_backfill as b,chat_history_backfill_operation as operation,product_shadow_read
from routers import chat,chat_history
def session():return {"id":1,"user_email":"owner","title":"Chat","created_at":datetime(2026,1,1),"updated_at":datetime(2026,1,2)}
def message():return {"id":2,"session_id":1,"user_email":"owner","role":"user","content":"hello","sql_query":None,"chart_type":None,"data":None,"route":None,"created_at":datetime(2026,1,1)}
class Tx:
 def __init__(self,ones=(),rows=()):self.ones=list(ones);self.rows=list(rows);self.calls=[]
 def execute(self,sql,params=None):self.calls.append(("execute",sql,params));return 1
 def query_one(self,sql,params=None):self.calls.append(("one",sql,params));return self.ones.pop(0) if self.ones else {"watermark":3}
 def query(self,sql,params=None):self.calls.append(("many",sql,params));return {"rows":self.rows.pop(0) if self.rows else []}
@contextlib.contextmanager
def transaction(tx):yield tx
class Tests(unittest.TestCase):
 def test_snapshot_binds_message_to_owned_session_and_reconciles(self):
  tx=Tx(rows=[[session()],[message()]])
  with patch.object(b.db,"transaction",return_value=transaction(tx)):snapshot=b.capture_snapshot()
  b.validate(snapshot);self.assertEqual([r.kind for r in snapshot.records],["chat_session","chat_message"])
  targets=[None,None,*({"document":r.document} for r in snapshot.records)]
  with patch.object(b.product_store,"read",side_effect=targets),patch.object(b.product_store,"transact") as transact:self.assertEqual(b.apply_and_reconcile(snapshot)["created"],2)
  self.assertEqual(transact.call_count,2)
 def test_orphan_message_fails_closed(self):
  doc=b.message_document(message());record=b.Record("chat_message","2","owner",doc,b.canonical(doc));snapshot=b.Snapshot(1,(record,),b.digest((record,)))
  with self.assertRaisesRegex(RuntimeError,"session is missing"):b.validate(snapshot)
 def test_create_writer_is_default_off_and_enabled_failure_is_atomic(self):
  ctx=SimpleNamespace(email="owner");body=chat_history.SessionCreate(title="Chat")
  with patch.dict("os.environ",{},clear=True),patch.object(chat_history.db,"query_one",return_value=session()),patch.object(chat_history.product_outbox,"enqueue") as enqueue:chat_history.create_session(body,ctx)
  enqueue.assert_not_called();tx=Tx(ones=[session()])
  with patch.dict("os.environ",{"KAVEON_CHAT_HISTORY_OUTBOX_ENABLED":"true"}),patch.object(chat_history.db,"transaction",return_value=transaction(tx)),patch.object(chat_history.product_outbox,"enqueue",side_effect=RuntimeError("outbox failed")):
   with self.assertRaisesRegex(RuntimeError,"outbox failed"):chat_history.create_session(body,ctx)
 def test_chat_writer_is_default_off_and_enabled_failure_is_atomic(self):
  with patch.dict("os.environ",{},clear=True),patch.object(chat.db,"execute",side_effect=RuntimeError("source failed")),patch.object(chat.product_outbox,"enqueue") as enqueue:
   chat._save_message(1,"owner","user","hello")
  enqueue.assert_not_called()
  tx=Tx(ones=[session(),message(),session()])
  with patch.dict("os.environ",{"KAVEON_CHAT_HISTORY_OUTBOX_ENABLED":"true"}),patch.object(chat.db,"transaction",return_value=transaction(tx)),patch.object(chat.product_outbox,"enqueue",side_effect=RuntimeError("outbox failed")) as enqueue:
   with self.assertRaisesRegex(RuntimeError,"outbox failed"):chat._save_message(1,"owner","user","hello")
  self.assertEqual(enqueue.call_args.kwargs["family"],"chat_messages")
  self.assertTrue(any("FOR UPDATE" in sql for kind,sql,_ in tx.calls if kind=="one"))
 def test_shadow_is_default_off_bounded_and_owner_scoped(self):
  self.assertFalse(product_shadow_read.observe_chat_session(session(),[message()],"owner")["enabled"])
  docs=[b.message_document(message()),b.session_document(session())]
  with patch.dict("os.environ",{"KAVEON_CHAT_HISTORY_SHADOW_READ_ENABLED":"true"}),patch.object(product_shadow_read.product_store,"read",side_effect=[{"document":docs[1]},{"document":docs[0]}]) as read:self.assertEqual(product_shadow_read.observe_chat_session(session(),[message()],"owner")["status"],"match")
  self.assertTrue(all(call.args[2]=="owner" for call in read.call_args_list))
 def test_checkpoint_resume_is_tamper_evident_and_apply_gated(self):
  tx=Tx(rows=[[session()],[message()]])
  with patch.object(b.db,"transaction",return_value=transaction(tx)):snapshot=b.capture_snapshot()
  with tempfile.TemporaryDirectory() as directory:
   path=Path(directory)/"chat.json"
   with patch.object(operation.backfill,"capture_snapshot",return_value=snapshot):operation.run(path,apply=False,resume=False)
   original=path.read_text();path.write_text(original.replace('"next_index":0','"next_index":1'))
   with self.assertRaisesRegex(RuntimeError,"identity"):operation.load(path)
   path.write_text(original)
   with self.assertRaisesRegex(RuntimeError,"KAVEON_CHAT_HISTORY_MIGRATION_ENABLED"):operation.run(path,apply=True,resume=True)
   with patch.dict("os.environ",{"KAVEON_CHAT_HISTORY_MIGRATION_ENABLED":"true"}),patch.object(operation.backfill,"apply_record"),patch.object(operation.backfill,"apply_and_reconcile",return_value={"family":"chat_history"}):self.assertTrue(operation.run(path,apply=True,resume=True)["checkpoint_complete"])
if __name__=="__main__":unittest.main()
