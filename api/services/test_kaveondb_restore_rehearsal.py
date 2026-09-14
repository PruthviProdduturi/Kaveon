import hashlib
import json

import pytest

from services import kaveondb_recovery_evidence as evidence
from services import kaveondb_restore_rehearsal as rehearsal


class Client:
    def __init__(self,values=None): self.values=dict(values or {});self.deleted=[]
    def read(self,path,max_bytes): return self.values.get(path)
    def create_if_absent_with_etag(self,path,value):
        if path in self.values: raise RuntimeError("precondition failed")
        self.values[path]=value;return '"etag-'+str(len(self.values))+'"'
    def delete_if_match(self,path,etag): self.deleted.append((path,etag));self.values.pop(path)


def fixture():
    records=[{"kind":"dataset","id":"one","revision":2,"document_sha256":"a"*64}]
    content=json.dumps(records,separators=(",",":")).encode()
    identity=evidence.state_identity(records)
    manifest={"schema_version":1,"backup_id":"b1",
        "immutable_prefix":"https://acct.dfs.core.windows.net/state/backups/b1/",
        **identity,"objects":[{"path":"state-inventory.json","etag":"source-etag","size":len(content),
                               "sha256":hashlib.sha256(content).hexdigest()}]}
    return manifest,content


def test_restore_is_create_only_verified_and_emits_cleanup_manifest():
    manifest,content=fixture();source=Client({"backups/b1/state-inventory.json":content});target=Client()
    result=rehearsal.execute(manifest,"https://acct.dfs.core.windows.net/state/restores/job-1/",source,target)
    assert result["restore_executed"] is True and result["restored_table_count"]==1
    assert target.values["restores/job-1/state-inventory.json"]==content
    cleanup=result["cleanup_manifest"]
    cleaned=rehearsal.cleanup(cleanup,target)
    assert cleaned=={"cleanup_executed":True,"deleted_object_count":1}


def test_existing_destination_fails_without_restore_claim():
    manifest,content=fixture();source=Client({"backups/b1/state-inventory.json":content})
    target=Client({"restores/job-1/state-inventory.json":b"existing"})
    with pytest.raises(RuntimeError,match="precondition"):
        rehearsal.execute(manifest,"https://acct.blob.core.windows.net/state/restores/job-1/",source,target)


def test_tampered_source_and_state_identity_fail_closed():
    manifest,content=fixture()
    with pytest.raises(RuntimeError,match="source verification"):
        rehearsal.execute(manifest,"https://acct.blob.core.windows.net/state/restores/job-1/",
            Client({"backups/b1/state-inventory.json":content+b"x"}),Client())
    manifest["state_sha256"]="f"*64
    with pytest.raises(RuntimeError,match="state identity"):
        rehearsal.execute(manifest,"https://acct.blob.core.windows.net/state/restores/job-1/",
            Client({"backups/b1/state-inventory.json":content}),Client())


def test_restore_prefix_must_be_separate_and_bounded():
    manifest,_=fixture()
    with pytest.raises(RuntimeError,match="restore prefix"):
        rehearsal.parse_prefix("https://acct.blob.core.windows.net/state/backups/b1/?sig=x","backups")
    manifest["objects"][0]["size"]=rehearsal.MAX_OBJECT_BYTES+1
    with pytest.raises(RuntimeError,match="byte bound"):
        rehearsal.execute(manifest,"https://acct.blob.core.windows.net/state/restores/job-1/",Client(),Client())
