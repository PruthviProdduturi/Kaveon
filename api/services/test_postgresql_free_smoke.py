import json,os,unittest
from datetime import datetime,timezone
from unittest.mock import patch
from services import postgresql_free_smoke as smoke

NOW=datetime(2026,9,14,20,tzinfo=timezone.utc)
class Response:
 def __init__(self,value,status=200):self.value=value;self.status_code=status;self.content=json.dumps(value).encode()
 def json(self):return self.value
class Client:
 def __init__(self,mutate=None):self.calls=[];self.mutate=mutate
 def request(self,method,url,headers,json=None):
  self.calls.append((method,url,headers,json));path=url.split("example.test",1)[1]
  values={
   "/api/health":{"status":"healthy","authority":"kaveondb","checks":{"postgresql":{"required":False}}},
   "/api/v1/catalog-sources":{"catalogSources":[{"id":"c1","name":"private"}]},
   "/api/v1/data-sources":{"dataSources":[{"id":"s1"}]},"/api/v1/datasets":[{"id":"7"}],"/api/v1/charts":[{"id":"8"}],"/api/v1/dashboards":[{"id":"9"}],
   "/api/v1/lab/query-history":{"history":[{"id":"q1","sql_text":"secret sql"}]},"/api/v1/lab/saved-queries":[{"id":"sq1"}],
   "/api/v1/user/recents":[{"id":"r1"}],"/api/v1/favorites":[{"id":"f1"}],"/api/v1/chat/history":{"sessions":[{"id":"10"}]},
   "/api/v1/datasets/7":{"id":"7"},"/api/v1/charts/8":{"id":"8"},"/api/v1/dashboards/9":{"id":"9"},
   "/api/v1/lab/saved-queries/sq1":{"id":"sq1"},"/api/v1/chat/history/10":{"id":"10","messages":[]},"/api/v1/theme":{"theme_color":"dark"},
   "/api/v1/datasets/7/dlm":{"dataset_id":"7","artifact":{"private":"value"}},"/api/v1/datasets/7/dlm/context":{"ok":True,"context":{"private":"value"}},
   "/api/v1/dlm/ask":{"ok":True,"answer":"private answer"},"/api/v1/chat":{"route":"dlm","answer":"private answer"}}
  value=values[path];status=200
  if self.mutate:value,status=self.mutate(path,value,status)
  return Response(value,status)
class Tests(unittest.TestCase):
 def collect(self,client):
  with patch.dict(os.environ,{"KAVEON_POSTGRESQL_FREE_SMOKE_ENABLED":"true","KAVEON_POSTGRESQL_FREE_SMOKE_ALLOWED_HOSTS":"example.test"}):return smoke.collect("https://example.test","probe@example.test","super-secret","show totals","7",client=client,now=NOW)
 def test_exercises_full_http_surface_and_emits_only_counts_and_digests(self):
  client=Client();report=self.collect(client);self.assertEqual(report["status"],"passed");self.assertEqual(report["check_count"],21);encoded=json.dumps(report);self.assertNotIn("private",encoded);self.assertNotIn("secret sql",encoded);self.assertNotIn("super-secret",encoded);self.assertEqual(len(report["state_sha256"]),64);self.assertTrue(all(call[2]["x-proxy-secret"]=="super-secret" for call in client.calls));self.assertEqual(client.calls[-2][0],"POST");self.assertEqual(client.calls[-1][0],"POST")
  self.assertIs(smoke.verify(report,now=NOW),report)
 def test_verifier_rejects_incomplete_stale_and_tampered_reports(self):
  report=self.collect(Client())
  incomplete=json.loads(json.dumps(report));incomplete["checks"].pop();incomplete["check_count"]-=1
  with self.assertRaisesRegex(RuntimeError,"incomplete check coverage"):smoke.verify(incomplete,now=NOW)
  stale=json.loads(json.dumps(report));stale["checked_at"]="2026-09-14T18:00:00Z";stale["state_sha256"]=smoke._digest({key:value for key,value in stale.items() if key!="state_sha256"})
  with self.assertRaisesRegex(RuntimeError,"not fresh"):smoke.verify(stale,now=NOW,max_age_minutes=30)
  tampered=json.loads(json.dumps(report));tampered["checks"][0]["count"]=99
  with self.assertRaisesRegex(RuntimeError,"digest mismatch"):smoke.verify(tampered,now=NOW)
 def test_requires_kaveondb_health_and_nonempty_migrated_families(self):
  def unhealthy(path,value,status):
   if path=="/api/health":value={"status":"healthy","authority":"postgresql","checks":{"postgresql":{"required":True}}}
   return value,status
  with self.assertRaisesRegex(RuntimeError,"PostgreSQL-free"):self.collect(Client(unhealthy))
  def empty(path,value,status):return ([] if path=="/api/v1/charts" else value),status
  with self.assertRaisesRegex(RuntimeError,"charts.*empty"):self.collect(Client(empty))
 def test_http_failure_dlm_and_chat_failure_are_closed(self):
  def forbidden(path,value,status):return (value,403) if path=="/api/v1/favorites" else (value,status)
  with self.assertRaisesRegex(RuntimeError,"HTTP 403"):self.collect(Client(forbidden))
  def dlm(path,value,status):return ({"ok":False},status) if path=="/api/v1/dlm/ask" else (value,status)
  with self.assertRaisesRegex(RuntimeError,"DLM ask"):self.collect(Client(dlm))
  def chat(path,value,status):return ({"route":"no_match"},status) if path=="/api/v1/chat" else (value,status)
  with self.assertRaisesRegex(RuntimeError,"chat did not serve"):self.collect(Client(chat))
 def test_disabled_invalid_identity_and_remote_http_are_rejected(self):
  with patch.dict(os.environ,{},clear=True),self.assertRaisesRegex(RuntimeError,"explicit enablement"):smoke.collect("https://example.test","a@b.test","s","q","7",client=Client())
  with patch.dict(os.environ,{"KAVEON_POSTGRESQL_FREE_SMOKE_ENABLED":"true"}):
   with self.assertRaisesRegex(RuntimeError,"trusted.*incomplete"):smoke.collect("https://localhost","invalid","s","q","7",client=Client())
   with self.assertRaisesRegex(RuntimeError,"plain HTTP"):smoke.collect("http://example.test","a@b.test","s","q","7",client=Client())
   with self.assertRaisesRegex(RuntimeError,"not explicitly allowed"):smoke.collect("https://evil.example","a@b.test","s","q","7",client=Client())
if __name__=="__main__":unittest.main()
