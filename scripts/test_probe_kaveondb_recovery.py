import importlib.util,json
from pathlib import Path
SCRIPT=Path(__file__).with_name("probe-kaveondb-recovery.py")
SPEC=importlib.util.spec_from_file_location("recovery_probe",SCRIPT);module=importlib.util.module_from_spec(SPEC);SPEC.loader.exec_module(module)

def test_restart_cli_outputs_only_observation(monkeypatch,tmp_path,capsys):
 manifest=tmp_path/"m.json";manifest.write_text("{}")
 expected={"postgresql_unavailable":True,"api_restarted":True}
 monkeypatch.setattr(module.probes,"restart",lambda _:expected)
 monkeypatch.setattr("sys.argv",[str(SCRIPT),"restart-recovery","--manifest",str(manifest)])
 assert module.main()==0 and json.loads(capsys.readouterr().out)==expected

def test_rollback_cli_failure_is_machine_readable(monkeypatch,tmp_path,capsys):
 manifest=tmp_path/"m.json";control=tmp_path/"c.json";manifest.write_text("{}");control.write_text("{}")
 monkeypatch.setattr(module.probes,"rollback",lambda *_:(_ for _ in ()).throw(RuntimeError("not restored")))
 monkeypatch.setattr("sys.argv",[str(SCRIPT),"rollback","--manifest",str(manifest),"--control",str(control)])
 assert module.main()==1 and json.loads(capsys.readouterr().out)=={"passed":False,"error":"not restored"}
