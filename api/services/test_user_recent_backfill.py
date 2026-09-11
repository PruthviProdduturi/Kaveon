import contextlib,sys,unittest
from datetime import datetime
from types import SimpleNamespace
from unittest.mock import patch
if "pyodbc" not in sys.modules:sys.modules["pyodbc"]=SimpleNamespace(Error=Exception)
from services import user_recent_backfill as b
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
if __name__=="__main__":unittest.main()
