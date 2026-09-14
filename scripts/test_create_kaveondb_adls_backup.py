import importlib.util,json
from pathlib import Path

SCRIPT=Path(__file__).with_name("create-kaveondb-adls-backup.py")
SPEC=importlib.util.spec_from_file_location("backup_cli",SCRIPT)
module=importlib.util.module_from_spec(SPEC);SPEC.loader.exec_module(module)

def test_cli_writes_manifest_then_prints_strict_summary(monkeypatch,tmp_path,capsys):
 output=tmp_path/"manifest.json";manifest={"schema_version":1}
 monkeypatch.setattr(module,"AzureArtifactClient",lambda *_:object())
 monkeypatch.setattr(module.kaveondb_backup,"product_inventory",lambda:([],{"snapshot_id":"s"}))
 monkeypatch.setattr(module.kaveondb_backup,"create",lambda *_:{"backup_id":"b","snapshot_id":"s","manifest":manifest})
 monkeypatch.setattr("sys.argv",[str(SCRIPT),"--account","a","--container","c","--active-prefix","active","--backup-id","b","--manifest-output",str(output)])
 assert module.main()==0
 assert json.loads(output.read_text())==manifest
 assert json.loads(capsys.readouterr().out)=={"backup_created":True,"backup_id":"b","snapshot_id":"s"}

def test_cli_failure_does_not_publish_manifest(monkeypatch,tmp_path,capsys):
 output=tmp_path/"manifest.json"
 monkeypatch.setattr(module,"AzureArtifactClient",lambda *_:object())
 monkeypatch.setattr(module.kaveondb_backup,"product_inventory",lambda:(_ for _ in ()).throw(RuntimeError("snapshot changed")))
 monkeypatch.setattr("sys.argv",[str(SCRIPT),"--account","a","--container","c","--active-prefix","active","--backup-id","b","--manifest-output",str(output)])
 assert module.main()==1 and not output.exists()
 assert json.loads(capsys.readouterr().out)["backup_created"] is False
