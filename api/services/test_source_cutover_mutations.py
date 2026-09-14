from types import SimpleNamespace
from unittest.mock import Mock

import pytest

from routers import catalog_sources, data_sources
from services import source_cutover_mutations as cutover


@pytest.fixture(autouse=True)
def enabled(monkeypatch):
    monkeypatch.setenv("KAVEONDB_READ_AUTHORITY_FAMILIES", "sources,activity,favorites")


def _forbid_postgres(monkeypatch, module):
    forbidden = Mock(side_effect=AssertionError("PostgreSQL must not be called"))
    for name in ("transaction", "query", "query_one", "execute"):
        monkeypatch.setattr(module.db, name, forbidden)


def test_source_and_audit_create_are_one_transaction(monkeypatch):
    monkeypatch.setattr("services.product_store.read", lambda *_: None)
    transact = Mock()
    monkeypatch.setattr("services.product_store.transact", transact)
    row = {"id":"one","name":"Lake","engine_catalog":"lake","storage_type":"adls_gen2",
           "storage_config":{"account":"a","container":"c","root_path":""},"data_format":"delta",
           "credential_kind":"managed_identity","credential_ref":None,"adapter_type":"native",
           "adapter_config":{},"lifecycle":"draft","created_by":"admin"}
    cutover.create("catalog_sources", row, "admin")
    mutations = transact.call_args.args[0]
    assert [(item.operation, item.kind) for item in mutations] == [("create","source"),("create","activity")]
    assert "credential" not in str(mutations[1].document).lower()


def test_source_update_uses_exact_revision_and_atomic_audit(monkeypatch):
    monkeypatch.setattr("services.product_store.read", lambda *_: {
        "revision":6,"document":{"source_id":"catalog-one","source_kind":"catalog","name":"Old"}})
    transact=Mock()
    monkeypatch.setattr("services.product_store.transact",transact)
    cutover.update("catalog-one",{"name":"New"},"admin")
    mutations=transact.call_args.args[0]
    assert (mutations[0].operation,mutations[0].expected_revision,mutations[1].kind)==("update",6,"activity")


def test_activity_cutover_never_calls_postgres(monkeypatch):
    _forbid_postgres(monkeypatch,catalog_sources)
    audit=Mock()
    monkeypatch.setattr(catalog_sources.source_cutover_mutations,"audit_only",audit)
    catalog_sources._audit("synchronized","one","Lake","admin",'{"revision":2}')
    audit.assert_called_once_with("synchronized","one","Lake","admin",{"revision":2})


def test_catalog_create_never_calls_postgres(monkeypatch):
    _forbid_postgres(monkeypatch,catalog_sources)
    monkeypatch.setattr(catalog_sources.source_cutover_mutations,"new_catalog_id",lambda:"one")
    create=Mock()
    monkeypatch.setattr(catalog_sources.source_cutover_mutations,"create",create)
    result=catalog_sources.create_catalog_source({"name":"Lake","engine_catalog":"lake","storage_type":"adls_gen2",
        "storage_config":{"account":"a","container":"c","root_path":""},"data_format":"delta",
        "credential_kind":"managed_identity","adapter_type":"native"},SimpleNamespace(email="admin",role="Admin"))
    assert result["catalogSource"]["id"]=="one"
    create.assert_called_once()


def test_data_create_places_secret_in_key_vault_and_metadata_in_kaveondb(monkeypatch):
    _forbid_postgres(monkeypatch,data_sources)
    monkeypatch.setattr(data_sources.source_cutover_mutations,"new_data_id",lambda:"42")
    vault=Mock();vault.set.return_value="https://test.vault.azure.net/secrets/source/version"
    monkeypatch.setattr(data_sources.source_secret_store,"SourceSecretStore",lambda:vault)
    create=Mock()
    monkeypatch.setattr(data_sources.source_cutover_mutations,"create",create)
    result=data_sources.create_data_source({"name":"DB","type":"PostgreSQL","connection_string":"raw-password",
        "database_name":"db","region":"WW"},SimpleNamespace(email="admin",role="Admin"))
    assert result["dataSource"]["id"]=="42"
    vault.set.assert_called_once_with("data","42","raw-password")
    row=create.call_args.args[1]
    assert row["secret_ref"].startswith("https://") and "connection_string" not in row


def test_source_cutover_requires_activity_and_fails_before_postgres(monkeypatch):
    monkeypatch.setenv("KAVEONDB_READ_AUTHORITY_FAMILIES","sources")
    _forbid_postgres(monkeypatch,catalog_sources)
    with pytest.raises(RuntimeError,match="requires activity"):
        catalog_sources.create_catalog_source({"name":"Lake","engine_catalog":"lake","storage_type":"adls_gen2",
            "storage_config":{"account":"a","container":"c","root_path":""}},SimpleNamespace(email="admin",role="Admin"))
