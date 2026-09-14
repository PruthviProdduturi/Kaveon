from unittest.mock import patch
from services import aks_retirement_live_probe as probe

def test_state_inventory_returns_content_free_records():
    records=[{"kind":"datasets","id":"1","revision":1,"document_sha256":"a"*64}]
    with patch.object(probe.kaveondb_backup,"product_inventory",return_value=(records,{"snapshot_id":"s"})) as inventory:
        assert probe.state_inventory()==records
    inventory.assert_called_once_with(actor="kaveon-retirement-probe")

def test_api_health_requires_matching_authority(monkeypatch):
    class Response:
        status=200;headers={"Content-Length":"100"}
        def __enter__(self):return self
        def __exit__(self,*_args):pass
        def read(self,_size):return b'{"status":"healthy","authority":"kaveondb","checks":{"postgresql":{"required":false}}}'
    monkeypatch.setattr(probe,"urlopen",lambda *_args,**_kwargs:Response())
    assert probe.api_health("kaveondb")=={"api_healthy":True,"authority":"kaveondb"}

def test_smoke_report_uses_injected_identity_and_environment_secret(monkeypatch):
    monkeypatch.setenv("KAVEON_POSTGRESQL_RESTART_REHEARSAL_MODE","true");monkeypatch.setenv("KAVEON_PROXY_SECRET","secret")
    monkeypatch.setattr(probe.postgresql_free_smoke,"collect",lambda *args:{"args":args})
    value=probe.smoke_report("owner@example.com","15","question")
    assert value["args"]==("http://127.0.0.1:8080","owner@example.com","secret","question","15")
