import hashlib,json,os,unittest
from datetime import datetime,timezone
from unittest.mock import patch
from services import dlm_generation_retirement as retirement
from services import postgresql_evidence_collector as collector

NOW=datetime(2026,9,14,18,tzinfo=timezone.utc)
def evidence():
 rows={table:index+1 for index,table in enumerate(retirement.TABLES)}
 return {"schema_version":1,"observed_at":"2026-09-14T17:40:00Z",
  "source":{"snapshot_id":"pg-7","watermark":45,"rows":rows,"schema_sha256":{table:"a"*64 for table in retirement.TABLES}},
  "deletion":{"writes_fenced":True,"deleted_rows":dict(rows),"remaining_rows":{table:0 for table in retirement.TABLES},"verified_at":"2026-09-14T17:55:00Z","target_snapshot_id":"kdb-9"}}
class Tests(unittest.TestCase):
 def build(self,value=None):
  verified={"passed":True,"bundle_sha256":"b"*64,"definition_count":9,"run_count":10,"target_snapshot_id":"kdb-9"}
  with patch.dict(os.environ,{"KAVEON_DLM_GENERATION_RETIREMENT_ENABLED":"true"}),patch.object(retirement.dlm_migration_evidence,"verify",return_value=verified):
   return retirement.build_report({"bundle":True},value or evidence(),now=NOW,max_age_hours=1)
 def test_emits_strict_report_bound_to_compiled_target(self):
  report=self.build();self.assertEqual(report["family"],"dlm_generation");self.assertEqual((report["source_count"],report["target_count"]),(0,0));self.assertEqual(set(report),collector.REPORT_KEYS);unsigned={k:v for k,v in report.items() if k!="report_sha256"};self.assertEqual(report["report_sha256"],hashlib.sha256(collector._canonical(unsigned)).hexdigest());self.assertIn("kdb-9",report["provenance"]["target_snapshot"])
 def test_disabled_and_bundle_verification_fail_before_success(self):
  with patch.dict(os.environ,{},clear=True),self.assertRaisesRegex(RuntimeError,"explicit enablement"):retirement.build_report({},evidence(),now=NOW,max_age_hours=1)
  with patch.dict(os.environ,{"KAVEON_DLM_GENERATION_RETIREMENT_ENABLED":"true"}),patch.object(retirement.dlm_migration_evidence,"verify",side_effect=RuntimeError("bundle mismatch")),self.assertRaisesRegex(RuntimeError,"bundle mismatch"):retirement.build_report({},evidence(),now=NOW,max_age_hours=1)
 def test_requires_exact_delete_fence_and_target_binding(self):
  cases=[]
  value=evidence();value["deletion"]["writes_fenced"]=False;cases.append((value,"not fenced"))
  value=evidence();value["deletion"]["deleted_rows"][retirement.TABLES[0]]=0;cases.append((value,"do not match"))
  value=evidence();value["deletion"]["remaining_rows"][retirement.TABLES[1]]=1;cases.append((value,"rows remain"))
  value=evidence();value["deletion"]["target_snapshot_id"]="other";cases.append((value,"does not match"))
  for value,message in cases:
   with self.subTest(message=message),self.assertRaisesRegex(RuntimeError,message):self.build(value)
 def test_rejects_missing_table_and_stale_evidence(self):
  value=evidence();value["source"]["rows"].pop(retirement.TABLES[0])
  with self.assertRaisesRegex(RuntimeError,"coverage"):self.build(value)
  value=evidence();value["observed_at"]="2026-09-14T15:00:00Z"
  with self.assertRaisesRegex(RuntimeError,"stale"):self.build(value)
if __name__=="__main__":unittest.main()
