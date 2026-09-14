import importlib.util,json
from pathlib import Path

SCRIPT=Path(__file__).with_name("probe-kaveondb-cutover.py")
SPEC=importlib.util.spec_from_file_location("cutover_probe",SCRIPT)
module=importlib.util.module_from_spec(SPEC);SPEC.loader.exec_module(module)


def test_cli_prints_only_observation(monkeypatch,tmp_path,capsys):
    expected={"source_snapshot":"a"*64,"target_snapshot":"b"*64,"family_probes":[],"mismatch_count":0}
    monkeypatch.setattr(module.probes,"shadow_parity",lambda *_args,**_kwargs:expected)
    monkeypatch.setattr("sys.argv",[str(SCRIPT),"shadow-parity","--reports",str(tmp_path)])
    assert module.main()==0 and json.loads(capsys.readouterr().out)==expected


def test_cli_failure_is_machine_readable(monkeypatch,capsys):
    monkeypatch.setattr(module.probes,"write_fence",lambda *_:(_ for _ in ()).throw(RuntimeError("not fenced")))
    monkeypatch.setattr("sys.argv",[str(SCRIPT),"write-fence","--deployment-revision","api@x"])
    assert module.main()==1
    assert json.loads(capsys.readouterr().out)=={"passed":False,"error":"not fenced"}
