import importlib.util
from pathlib import Path
import pytest

SCRIPT=Path(__file__).with_name("benchmark-time-to-answer.py")
SPEC=importlib.util.spec_from_file_location("benchmark_time_to_answer",SCRIPT)
module=importlib.util.module_from_spec(SPEC);SPEC.loader.exec_module(module)

def test_next_uri_stays_on_configured_trino_statement_origin():
    base="https://trino.kaveon.svc.cluster.local:8443"
    good=base+"/v1/statement/executing/abc/1?slug=x"
    assert module.validated_next_uri(base,good)==good
    for value in ("https://evil.example/v1/statement/abc",base+"/ui/",base+"/v1/statement/abc#fragment",
                  "https://user:pass@trino.kaveon.svc.cluster.local:8443/v1/statement/abc"):
        with pytest.raises(RuntimeError,match="unsafe next URI"):
            module.validated_next_uri(base,value)
