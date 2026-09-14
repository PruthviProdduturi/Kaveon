"""Retirement-mode DLM workflows may never fall back to PostgreSQL metadata."""
import contextlib
from types import SimpleNamespace
from unittest.mock import Mock, patch

import pytest
from fastapi import HTTPException

from dlm import engine
from routers import context as context_router
from routers import dlm as dlm_router


DATASET = {
    "id": "24", "dataset_name": "Orders", "database_name": "lake",
    "schema_name": "silver", "fact_table": "orders", "created_by": "owner",
    "columns": [], "dimensions": [], "metrics": [], "visibility": "published",
}
ARTIFACT = {
    "dataset_id": "24", "version": 1, "status": "ready", "source_hash": "source",
    "built_at": "2026-09-14T20:00:00Z", "manifest": {"name": "Orders", "context_spec": {}},
    "stats_rollup": {}, "usage_rollup": {}, "values_indexed": 0,
    "compiled_context": {"values": [], "answers": [], "sketches": [], "router": {}, "curation": {}},
}


@pytest.fixture
def no_postgres():
    forbidden = Mock(side_effect=AssertionError("PostgreSQL metadata operation reached DLM retirement workflow"))
    patches = [patch.object(engine.meta, name, forbidden) for name in ("query", "query_one", "execute", "transaction")]
    with contextlib.ExitStack() as stack:
        for item in patches:
            stack.enter_context(item)
        stack.enter_context(patch("services.postgresql_retirement_runtime.requested", return_value=True))
        yield forbidden


def test_generate_profile_curate_context_and_ask_are_postgresql_free(no_postgres):
    ctx = SimpleNamespace(email="owner", role="Admin")
    generate_stubs = {
        "_analyze_tables": None, "_value_inventory": [{"id": "v", "dataset_id": "24",
            "element_key": "region", "value_text": "West", "value_norm": "west",
            "key_column": "region", "key_value": "West", "freq": 1.0, "source": "test"}], "_usage_rollup": {},
        "_stats_rollup": {}, "_native_row_counts": {}, "_manifest": ARTIFACT["manifest"],
        "_persist_value_index": None, "_upsert_artifact": None, "_upsert_router": None,
        "_effective_spec": {}, "_curate_linked_dashboards": 0,
    }
    with contextlib.ExitStack() as stack:
        stack.enter_context(patch.object(engine.datasets_svc, "get_dataset_by_id", return_value=DATASET))
        stack.enter_context(patch.object(engine, "get_dlm", return_value=None))
        profile = stack.enter_context(patch.object(engine.profiler, "build_context",
                                                   side_effect=AssertionError("legacy profile reached")))
        publish = stack.enter_context(patch("services.dlm_generation_cutover.publish"))
        for name, value in generate_stubs.items():
            stack.enter_context(patch.object(engine, name, lambda *args, _value=value, **kwargs: _value))
        result = dlm_router.generate("24", True, ctx)
    assert result["rebuilt"] is True
    profile.assert_not_called()
    publish.assert_called_once()

    def precompute(dataset_id, *_args, **_kwargs):
        engine._store_answer(dataset_id, "Revenue", "region|channel", ["region", "channel", "Revenue"], [], "now")
        return 1

    with patch.object(engine, "_dashboard_combos", return_value={"24": {("region", "channel")}}), \
         patch.object(engine.datasets_svc, "get_dataset_by_id", return_value={**DATASET, "metrics": [{"name": "Revenue", "expression": "SUM(revenue)"}]}), \
         patch.object(engine, "_precompute_n_dim", side_effect=precompute), \
         patch.object(engine, "get_dlm", return_value=ARTIFACT), \
         patch("services.dlm_generation_cutover.publish") as curate_publish:
        curated = dlm_router.curate_dashboard("9", ctx)
    assert curated["answers_stored"] == 1
    assert curate_publish.call_args.args[0]["compiled_context"]["answers"][0]["metric_name"] == "Revenue"

    with patch.object(engine, "get_dlm", return_value=ARTIFACT), \
         patch("services.dlm_generation_cutover.publish") as save_publish:
        assert dlm_router.get_context("24", ctx)["ok"] is True
        assert dlm_router.put_context("24", dlm_router.CurationBody(default_metric="Revenue"), ctx)["ok"] is True
    save_publish.assert_called_once()

    with patch.object(engine, "get_dlm", return_value=ARTIFACT), \
         patch("services.product_store.list_records", return_value=[]):
        answer = dlm_router.ask(dlm_router.AskBody(question="SELECT 1"), ctx)
    assert answer["reason"] == "out_of_scope"
    no_postgres.assert_not_called()


def test_legacy_context_profiler_fails_before_postgresql(no_postgres):
    ctx = SimpleNamespace(email="owner", role="Admin")
    with patch.object(context_router.profiler, "build_context",
                      side_effect=AssertionError("legacy profiler reached")) as profile:
        with pytest.raises(HTTPException) as error:
            context_router.build_context(context_router.BuildBody(database="lake"), ctx)
    assert error.value.status_code == 503
    profile.assert_not_called()
    no_postgres.assert_not_called()


def test_retirement_discovery_uses_admin_storage_read_then_caller_visibility(no_postgres):
    state = engine._RetirementServingState("viewer", "Viewer")
    token = engine._RETIREMENT_SERVING.set(state)
    try:
        with patch("services.product_store.list_records", return_value=[{"id": "24"}]) as records, \
             patch.object(engine, "get_dlm", return_value=ARTIFACT) as visible_read:
            assert engine._serving_artifacts() == [ARTIFACT]
        records.assert_called_once_with("dataset", "viewer", "Admin", max_records=1000)
        visible_read.assert_called_once_with("24", "viewer", "Viewer")
    finally:
        engine._RETIREMENT_SERVING.reset(token)
    no_postgres.assert_not_called()
