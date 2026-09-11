import os,sys,unittest
from types import SimpleNamespace
from unittest.mock import patch
if "pyodbc" not in sys.modules:sys.modules["pyodbc"]=SimpleNamespace(Error=Exception)
from services import product_shadow_read as shadow
SOURCE={"id":"q1","name":"Trips","description":None,"sql":"SELECT 1","created_at":"2026-01-01","updated_at":"2026-01-02","created_by":"owner","modified_by":"owner"}
class Tests(unittest.TestCase):
 def test_default_off_and_owner_scoped_point(self):
  self.assertFalse(shadow.observe_saved_query(SOURCE,"owner")["enabled"])
  with patch.dict(os.environ,{"KAVEON_SAVED_QUERY_SHADOW_READ_ENABLED":"true"}),patch.object(shadow.product_store,"read",return_value={"document":SOURCE}) as read:
   self.assertEqual(shadow.observe_saved_query(SOURCE,"owner")["status"],"match")
  self.assertEqual(read.call_args.args[2],"owner")
 def test_list_is_bounded_and_content_free(self):
  with patch.dict(os.environ,{"KAVEON_SAVED_QUERY_SHADOW_READ_ENABLED":"true"}):
   result=shadow.observe_saved_query_list([SOURCE]*(shadow.MAX_SHADOW_LIST_RECORDS+1),"owner")
  self.assertEqual(result["status"],"skipped_limit");self.assertNotIn("sql",result)
if __name__=="__main__":unittest.main()
