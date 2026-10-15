import json
from datetime import datetime,timezone
from unittest.mock import Mock

import pytest

from services import live_cutover_probes as probes
from services import postgresql_retirement_gate as gate
from services import postgresql_write_fence as fence


def report(family):
    return {"status":"passed","source_count":2,"target_count":2,
        "reconciled_at":"2026-09-14T18:00:00Z",
        "checks":{name:True for name in gate.REQUIRED_CHECKS},
        "provenance":{"source_snapshot":"pg:"+family,"target_snapshot":"kdb:"+family}}


def test_shadow_parity_validates_every_family_and_emits_no_content(monkeypatch,tmp_path):
    for family in gate.AUTHORITY_FAMILIES:(tmp_path/f"{family}.json").write_text("{}")
    monkeypatch.setattr(probes.collector,"_load_report",lambda _path,family:report(family))
    result=probes.shadow_parity(tmp_path,now=datetime(2026,9,14,19,tzinfo=timezone.utc))
    assert result["mismatch_count"]==0
    assert [item["family"] for item in result["family_probes"]]==sorted(gate.AUTHORITY_FAMILIES)
    assert set(result)=={"source_snapshot","target_snapshot","family_probes","mismatch_count"}


def test_shadow_parity_fails_closed_on_missing_or_mismatch(monkeypatch,tmp_path):
    for family in list(gate.AUTHORITY_FAMILIES)[1:]:(tmp_path/f"{family}.json").write_text("{}")
    with pytest.raises(RuntimeError,match="exactly once"):probes.shadow_parity(tmp_path)
    family=next(iter(gate.AUTHORITY_FAMILIES));(tmp_path/f"{family}.json").write_text("{}")
    monkeypatch.setattr(probes.collector,"_load_report",lambda _path,name:{**report(name),"target_count":1} if name==family else report(name))
    with pytest.raises(RuntimeError,match="mismatches"):probes.shadow_parity(tmp_path,now=datetime(2026,9,14,19,tzinfo=timezone.utc))


def test_write_fence_executes_read_and_requires_rejection_per_family(monkeypatch):
    monkeypatch.setenv("METADATA_DB_TYPE","postgresql");monkeypatch.setenv(fence.ENVIRONMENT_KEY,"true")
    query=Mock(return_value={"rows":[{"retirement_fence_read_probe":1}]});monkeypatch.setattr(probes.db,"query",query)
    attempted=[]
    def execute(sql):
        attempted.append(sql);fence.assert_allowed(sql,"postgresql")
    monkeypatch.setattr(probes.db,"execute",execute)
    result=probes.write_fence("api@abc")
    # Every table of every family, not one per family. Only `tables[0]` used
    # to be probed, so ten of the authority tables were never touched while
    # the receipt claimed the whole family — a fence regression on any of them
    # read as a pass.
    every_table=[table for tables in gate.AUTHORITY_FAMILIES.values() for table in tables]
    assert len(attempted)==len(every_table)
    assert {sql.split(" ")[2] for sql in attempted}==set(every_table)
    assert len(result["family_probes"])==len(gate.AUTHORITY_FAMILIES)


def test_a_fence_regression_on_a_non_first_table_is_not_a_pass(monkeypatch):
    """The whole point of probing past `tables[0]`."""
    monkeypatch.setenv("METADATA_DB_TYPE","postgresql");monkeypatch.setenv(fence.ENVIRONMENT_KEY,"true")
    monkeypatch.setattr(probes.db,"query",lambda *_:{"rows":[{"x":1}]})
    leaky="user_ai_keys"
    def execute(sql):
        if leaky in sql:return None
        fence.assert_allowed(sql,"postgresql")
    monkeypatch.setattr(probes.db,"execute",execute)
    with pytest.raises(RuntimeError,match=f"allowed ai_configuration.{leaky}"):
        probes.write_fence("api@abc")


def test_write_fence_fails_if_any_mutation_reaches_database(monkeypatch):
    monkeypatch.setenv("METADATA_DB_TYPE","postgresql");monkeypatch.setenv(fence.ENVIRONMENT_KEY,"true")
    monkeypatch.setattr(probes.db,"query",lambda *_:{"rows":[{"x":1}]})
    monkeypatch.setattr(probes.db,"execute",lambda *_:None)
    with pytest.raises(RuntimeError,match="allowed"):probes.write_fence("api@abc")
