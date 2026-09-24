import hashlib,json
import pytest
from services import kaveondb_backup as backup
from services import kaveondb_recovery_evidence as evidence

class Client:
 def __init__(self,objects):self.objects=dict(objects);self.created={};self.changed=False
 def list(self,prefix,bound):
  values=[{"path":path,"etag":"e-"+path,"size":len(value)} for path,value in self.objects.items() if path.startswith(prefix.rstrip("/")+"/")]
  if self.changed:values[0]={**values[0],"etag":"changed"}
  self.changed=True;return values
 def read(self,path,size):return self.created.get(path,self.objects.get(path))
 def create_if_absent_with_etag(self,path,value):
  if path in self.created:raise RuntimeError("exists")
  self.created[path]=value;return "etag-"+str(len(self.created))
 def _url(self,path):return "https://acct.blob.core.windows.net/state/"+path

def test_create_enumerates_twice_and_publishes_manifest_last():
 records=[{"kind":"dataset","id":"1","revision":2,"document_sha256":"a"*64}];state={**evidence.state_identity(records),"snapshot_id":"snapshot-7"}
 client=Client({"transactions/current/head.json":b"head","transactions/current/snapshots/7.json":b"snapshot"})
 client.changed=False
 # Stable list implementation for this success path.
 original=client.list;first=original("transactions/current",10000);client.changed=False
 client.list=lambda *_:first
 result=backup.create("transactions/current","b1",client,client,records,state)
 assert list(client.created)[-1]=="backups/b1/manifest.json"
 assert result["snapshot_id"]=="snapshot-7" and result["record_count"]==1
 assert "backups/b1/state-inventory.json" in client.created

def test_changed_active_prefix_fails_before_manifest_publication():
 records=[];state={**evidence.state_identity(records),"snapshot_id":"empty"}
 client=Client({"active/head":b"head"})
 with pytest.raises(RuntimeError,match="changed during backup"):backup.create("active","b1",client,client,records,state)
 assert "backups/b1/manifest.json" not in client.created

def test_stale_state_identity_fails_before_copy():
 records=[{"kind":"dataset","id":"1","revision":1,"document_sha256":"a"*64}]
 client=Client({"active/head":b"head"})
 state={"snapshot_id":"snapshot-1","record_count":1,"state_sha256":"b"*64}
 with pytest.raises(RuntimeError,match="does not match supplied identity"):
  backup.create("active","b1",client,client,records,state)
 assert not client.created

def test_inventory_requires_one_snapshot_across_kinds(monkeypatch):
 count=0
 def listed(kind,*_args,**_kwargs):
  nonlocal count;count+=1
  return [],"snapshot-a" if count==1 else "snapshot-b"
 monkeypatch.setattr(backup.product_store,"list_records_snapshot",listed)
 with pytest.raises(RuntimeError,match="snapshot changed"):backup.product_inventory()
