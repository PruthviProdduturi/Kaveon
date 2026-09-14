import importlib.util
import json
from pathlib import Path


SCRIPT=Path(__file__).with_name("rehearse-kaveondb-adls-restore.py")
SPEC=importlib.util.spec_from_file_location("restore_rehearsal",SCRIPT)
module=importlib.util.module_from_spec(SPEC);SPEC.loader.exec_module(module)


def test_cli_publishes_cleanup_only_after_success(monkeypatch,tmp_path,capsys):
    manifest=tmp_path/"manifest.json";cleanup=tmp_path/"cleanup.json"
    manifest.write_text(json.dumps({"immutable_prefix":"https://a.blob.core.windows.net/c/backups/b/"}),encoding="utf-8")
    result={"backup_id":"b","restore_executed":True,"cleanup_manifest":{"schema_version":1,"restore_prefix":"x","objects":[]}}
    monkeypatch.setattr(module,"_client",lambda _prefix:object())
    monkeypatch.setattr(module.rehearsal,"execute",lambda *_args:dict(result))
    monkeypatch.setattr("sys.argv",[str(SCRIPT),"restore","--manifest",str(manifest),
        "--restore-prefix","https://a.blob.core.windows.net/c/restores/r/","--cleanup-manifest",str(cleanup)])
    assert module.main()==0
    output=json.loads(capsys.readouterr().out)
    assert output=={"backup_id":"b","restore_executed":True}
    assert json.loads(cleanup.read_text())["schema_version"]==1


def test_cli_failure_never_claims_restore(monkeypatch,tmp_path,capsys):
    manifest=tmp_path/"manifest.json";cleanup=tmp_path/"cleanup.json"
    manifest.write_text(json.dumps({"immutable_prefix":"https://a.blob.core.windows.net/c/backups/b/"}),encoding="utf-8")
    monkeypatch.setattr(module,"_client",lambda _prefix:object())
    monkeypatch.setattr(module.rehearsal,"execute",lambda *_args:(_ for _ in ()).throw(RuntimeError("copy failed")))
    monkeypatch.setattr("sys.argv",[str(SCRIPT),"restore","--manifest",str(manifest),
        "--restore-prefix","https://a.blob.core.windows.net/c/restores/r/","--cleanup-manifest",str(cleanup)])
    assert module.main()==1
    output=json.loads(capsys.readouterr().out)
    assert output["restore_executed"] is False and not cleanup.exists()
