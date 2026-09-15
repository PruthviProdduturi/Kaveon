"""Black-box HTTP qualification for a PostgreSQL-free API restart."""
from __future__ import annotations
import hashlib,json,os
from datetime import datetime,timezone
from urllib.parse import urlparse
import httpx

SCHEMA_VERSION=1; MAX_RESPONSE_BYTES=8*1024*1024; REQUIRED_ROLE="Admin"
CHECK_NAMES=("health","catalog_sources","data_sources","datasets","charts","dashboards","query_history","saved_queries","recents","favorites","chat_history","datasets_point","charts_point","dashboards_point","saved_query_point","chat_history_point","theme","dlm_get","dlm_context","dlm_ask","chat_serving")
NONEMPTY_CHECKS=frozenset(("catalog_sources","datasets","charts","dashboards","query_history"))
ABSENT_POINT_CHECKS=frozenset(("saved_query_point","chat_history_point"))

def _canonical(value):return json.dumps(value,sort_keys=True,separators=(",",":"),ensure_ascii=False).encode()
def _digest(value):return hashlib.sha256(_canonical(value)).hexdigest()
def _sha256(value,label):
 if not isinstance(value,str) or len(value)!=64 or any(c not in "0123456789abcdef" for c in value):raise RuntimeError(f"{label} is not a lowercase SHA-256 digest")
 return value

def verify(report,*,now=None,max_age_minutes=60):
 """Validate that a restart probe is the complete, fresh HTTP smoke report."""
 if not isinstance(report,dict) or set(report)!={"schema_version","status","checked_at","authority","check_count","checks","state_sha256"}:raise RuntimeError("PostgreSQL-free smoke report schema is invalid")
 if report["schema_version"]!=SCHEMA_VERSION or report["status"]!="passed" or report["authority"]!="kaveondb":raise RuntimeError("PostgreSQL-free smoke report did not prove KaveonDB authority")
 if type(max_age_minutes) is not int or max_age_minutes<1:raise RuntimeError("PostgreSQL-free smoke freshness bound is invalid")
 try:checked=datetime.fromisoformat(report["checked_at"].replace("Z","+00:00"))
 except (AttributeError,ValueError) as error:raise RuntimeError("PostgreSQL-free smoke timestamp is invalid") from error
 current=now or datetime.now(timezone.utc)
 if checked.tzinfo is None or current.tzinfo is None:raise RuntimeError("PostgreSQL-free smoke timestamp must be timezone-aware")
 age=(current.astimezone(timezone.utc)-checked.astimezone(timezone.utc)).total_seconds()
 if age<0 or age>max_age_minutes*60:raise RuntimeError("PostgreSQL-free smoke report is not fresh")
 checks=report["checks"]
 if not isinstance(checks,list) or report["check_count"]!=len(CHECK_NAMES) or len(checks)!=len(CHECK_NAMES):raise RuntimeError("PostgreSQL-free smoke report has incomplete check coverage")
 names=[]
 for item in checks:
  if not isinstance(item,dict) or set(item)!={"name","status","count","state_sha256"}:raise RuntimeError("PostgreSQL-free smoke check schema is invalid")
  names.append(item["name"])
  status=item["status"]
  if type(status) is not int or (not 200<=status<300 and not (item["name"] in ABSENT_POINT_CHECKS and status==404 and item["count"]==0)):raise RuntimeError("PostgreSQL-free smoke check did not succeed")
  if type(item["count"]) is not int or item["count"]<0 or (item["name"] in NONEMPTY_CHECKS and item["count"]<1):raise RuntimeError("PostgreSQL-free smoke check count is invalid")
  _sha256(item["state_sha256"],"PostgreSQL-free smoke check state")
 if tuple(names)!=CHECK_NAMES or len(names)!=len(set(names)):raise RuntimeError("PostgreSQL-free smoke report check identities are invalid")
 claimed=_sha256(report["state_sha256"],"PostgreSQL-free smoke report state")
 unsigned={key:value for key,value in report.items() if key!="state_sha256"}
 if claimed!=_digest(unsigned):raise RuntimeError("PostgreSQL-free smoke report digest mismatch")
 return report
def _list(value,*keys):
 if isinstance(value,list):return value
 if isinstance(value,dict):
  for key in keys:
   if isinstance(value.get(key),list):return value[key]
 raise RuntimeError("smoke response did not contain the required list")
def _id(item,label):
 value=item.get("id") if isinstance(item,dict) else None
 if value is None or not str(value):raise RuntimeError(f"{label} did not expose a point-read identity")
 return str(value)

class Probe:
 def __init__(self,base_url,email,proxy_secret,*,ca_cert=None,client=None):
  parsed=urlparse(base_url)
  if parsed.scheme not in {"http","https"} or not parsed.hostname or parsed.username or parsed.password or parsed.query or parsed.fragment:
   raise RuntimeError("smoke API URL is invalid")
  local=parsed.hostname in {"localhost","127.0.0.1","::1"} or parsed.hostname.endswith(".svc.cluster.local")
  allowed={item.strip().lower() for item in os.getenv("KAVEON_POSTGRESQL_FREE_SMOKE_ALLOWED_HOSTS","").split(",") if item.strip()}
  if parsed.scheme!="https" and not local:
   raise RuntimeError("plain HTTP smoke probes are restricted to loopback or cluster DNS")
  if not local and parsed.hostname.lower() not in allowed:
   raise RuntimeError("smoke API host is not explicitly allowed")
  if not email or "@" not in email or not proxy_secret:raise RuntimeError("trusted smoke identity is incomplete")
  self.base=base_url.rstrip("/");self.headers={"x-proxy-secret":proxy_secret,"x-user-email":email.lower(),"x-user-role":REQUIRED_ROLE,"x-user-roles":REQUIRED_ROLE,"x-user-name":"Retirement probe"}
  self.client=client or httpx.Client(timeout=60,verify=ca_cert or True,trust_env=False)
 def request(self,name,path,*,method="GET",body=None,allow_absent=False):
  response=self.client.request(method,self.base+path,headers=self.headers,json=body)
  if (response.status_code<200 or response.status_code>=300) and not (allow_absent and response.status_code==404):raise RuntimeError(f"{name} failed with HTTP {response.status_code}")
  if len(response.content)>MAX_RESPONSE_BYTES:raise RuntimeError(f"{name} response exceeds its byte bound")
  try:value=response.json()
  except ValueError as error:raise RuntimeError(f"{name} returned invalid JSON") from error
  return value,{"name":name,"status":response.status_code,"count":0 if response.status_code==404 else 1,"state_sha256":_digest(value)}

def collect(base_url,email,proxy_secret,question,dataset_id,*,ca_cert=None,client=None,now=None):
 if os.getenv("KAVEON_POSTGRESQL_FREE_SMOKE_ENABLED")!="true":raise RuntimeError("PostgreSQL-free smoke collection requires explicit enablement")
 if not question or len(question)>500:raise RuntimeError("a bounded DLM smoke question is required")
 if not dataset_id or len(str(dataset_id))>255:raise RuntimeError("a bounded DLM dataset identity is required")
 probe=Probe(base_url,email,proxy_secret,ca_cert=ca_cert,client=client);checks=[]
 health,item=probe.request("health","/api/health")
 if health.get("status")!="healthy" or health.get("authority")!="kaveondb" or health.get("checks",{}).get("postgresql",{}).get("required") is not False:raise RuntimeError("health did not prove PostgreSQL-free KaveonDB authority")
 checks.append(item)
 lists={}
 for name,path,keys in (
  ("catalog_sources","/api/v1/catalog-sources",("catalogSources",)),("data_sources","/api/v1/data-sources",("dataSources","sources")),
  ("datasets","/api/v1/datasets",()),("charts","/api/v1/charts",()),("dashboards","/api/v1/dashboards",()),
  ("query_history","/api/v1/lab/query-history",("history","queries","rows")),("saved_queries","/api/v1/lab/saved-queries",("queries","savedQueries","rows")),
  ("recents","/api/v1/user/recents",("recents","items","rows")),("favorites","/api/v1/favorites",("favorites","rows")),
  ("chat_history","/api/v1/chat/history",("sessions",))):
  value,item=probe.request(name,path);rows=_list(value,*keys);item["count"]=len(rows);checks.append(item);lists[name]=rows
 for required in NONEMPTY_CHECKS:
  if not lists[required]:raise RuntimeError(f"{required} smoke coverage is empty")
 for family,path in (("datasets","/api/v1/datasets/"),("charts","/api/v1/charts/"),("dashboards","/api/v1/dashboards/")):
  _,item=probe.request(f"{family}_point",path+_id(lists[family][0],family));checks.append(item)
 if lists["saved_queries"]:
  saved_id=_id(lists["saved_queries"][0],"saved_queries");_,item=probe.request("saved_query_point","/api/v1/lab/saved-queries/"+saved_id)
 else: _,item=probe.request("saved_query_point","/api/v1/lab/saved-queries/__postgresql_free_absence_probe__",allow_absent=True)
 checks.append(item)
 if lists["chat_history"]:
  session_id=_id(lists["chat_history"][0],"chat_history");_,item=probe.request("chat_history_point","/api/v1/chat/history/"+session_id)
 else: _,item=probe.request("chat_history_point","/api/v1/chat/history/__postgresql_free_absence_probe__",allow_absent=True)
 checks.append(item)
 theme,item=probe.request("theme","/api/v1/theme");checks.append(item)
 dataset_id=str(dataset_id)
 if dataset_id not in {_id(item,"datasets") for item in lists["datasets"]}:raise RuntimeError("DLM smoke dataset is absent from visible datasets")
 for name,path in (("dlm_get",f"/api/v1/datasets/{dataset_id}/dlm"),("dlm_context",f"/api/v1/datasets/{dataset_id}/dlm/context")):
  _,item=probe.request(name,path);checks.append(item)
 ask,item=probe.request("dlm_ask","/api/v1/dlm/ask",method="POST",body={"question":question,"limit":3})
 if not isinstance(ask,dict) or ask.get("ok") is not True:raise RuntimeError("DLM ask did not serve an answer")
 checks.append(item)
 chat_body={"question":question}
 if dataset_id.isdecimal():chat_body["dataset_id"]=int(dataset_id)
 chat,item=probe.request("chat_serving","/api/v1/chat",method="POST",body=chat_body)
 if not isinstance(chat,dict) or chat.get("route") in {None,"error","no_match"}:raise RuntimeError("chat did not serve from migrated product state")
 checks.append(item)
 at=(now or datetime.now(timezone.utc)).astimezone(timezone.utc).isoformat().replace("+00:00","Z")
 report={"schema_version":SCHEMA_VERSION,"status":"passed","checked_at":at,"authority":"kaveondb","check_count":len(checks),"checks":checks}
 report["state_sha256"]=_digest(report);return verify(report,now=now)
