"""Representative API workflows must remain PostgreSQL-free after retirement."""
from types import SimpleNamespace
from unittest.mock import Mock

import pytest
from fastapi import Response

import database.metadata as metadata
from routers import catalog, catalog_sources, data_sources, lab, sql
from services import engine_bridge, product_read_authority


@pytest.fixture(autouse=True)
def retired(monkeypatch):
    monkeypatch.setenv("KAVEON_POSTGRESQL_RETIREMENT_MODE","true")
    monkeypatch.setenv(product_read_authority.ENVIRONMENT_KEY,",".join(product_read_authority.SUPPORTED_FAMILIES))
    forbidden=Mock(side_effect=AssertionError("PostgreSQL operation reached retirement workflow"))
    for name in ("query","query_one","execute","transaction"):
        monkeypatch.setattr(metadata,name,forbidden)
    return forbidden


def documents(family,*_args):
    if family=="sources":return [
        {"source_kind":"catalog","source_id":"catalog-lake","name":"Lake","catalog_identity":"lake",
         "source_type":"adls_gen2","storage_config":{},"adapter_config":{},"adapter_type":"native","lifecycle":"active"},
        {"source_kind":"data","source_id":"data-7","name":"Warehouse","database_name":"warehouse",
         "source_type":"PostgreSQL","region":"WW","is_active":True,"favorite":True}]
    if family=="favorites":return []
    if family=="datasets":return [{"id":13,"dataset_name":"Trips","database_name":"lake",
        "schema_name":"silver","fact_table":"trips","visibility":"published"}]
    if family=="charts":return [{"id":21,"name":"Trips by day","dataset_id":13,"visibility":"published"}]
    if family=="dashboards":return [{"id":31,"name":"Mobility","slug":"mobility","charts":[21],"visibility":"published"}]
    if family=="dlm_definitions":return [{"id":"dlm-13","dataset_id":13,"status":"ready"}]
    return []


def point(family,record_id,*_args):
    return next((item for item in documents(family) if item.get("source_id")==record_id),None)


def test_studio_lab_source_tree_and_database_picker_do_not_touch_postgres(monkeypatch,retired):
    monkeypatch.setattr(product_read_authority,"list_documents",documents)
    monkeypatch.setattr(product_read_authority,"read_document",point)
    # The picker keeps the sources the Engine's catalog list grants the principal.
    monkeypatch.setattr(engine_bridge,"catalogs",lambda actor,role:{"catalogs":["lake"]})
    response=Response();ctx=SimpleNamespace(email="alice",role="Admin")
    assert lab.list_databases(response,"alice")["databases"][0]["database"]=="warehouse"
    assert lab.list_engine_sources(response,ctx)["sources"][0]["catalog"]=="lake"
    assert lab._engine_source("lake")["engine_catalog"]=="lake"
    retired.assert_not_called()


def test_data_source_detail_favorite_and_connection_test_do_not_touch_postgres(monkeypatch,retired):
    monkeypatch.setattr(product_read_authority,"list_documents",documents)
    monkeypatch.setattr(product_read_authority,"read_document",point)
    monkeypatch.setattr("services.favorites.create_favorite",Mock(return_value={}))
    monkeypatch.setattr("services.favorites.delete_favorite",Mock(return_value=True))
    response=Response()
    assert data_sources.get_favorite_data_source("alice")["dataSource"]["id"]=="7"
    assert data_sources.get_data_source("7",response,"alice")["dataSource"]["name"]=="Warehouse"
    assert data_sources.test_data_source("7","alice")["database"]=="warehouse"
    data_sources.set_ds_favorite("7","alice");data_sources.remove_ds_favorite("7","alice")
    retired.assert_not_called()


def test_catalog_engine_sync_uses_kaveondb_source_and_activity(monkeypatch,retired):
    monkeypatch.setattr(product_read_authority,"read_document",point)
    monkeypatch.setattr(catalog_sources,"_audit",Mock())
    monkeypatch.setattr("services.engine_bridge.sync_catalog",lambda row,user,revision:{"catalog":{"id":"lake","revision":revision or 1}})
    result=catalog_sources.sync_engine_catalog("lake",{"expected_revision":2},SimpleNamespace(email="alice",role="Admin"))
    assert result["catalog"]["revision"]==2
    retired.assert_not_called()


def test_catalog_usage_projection_is_postgresql_free(monkeypatch,retired):
    monkeypatch.setattr(product_read_authority,"list_documents",documents)
    monkeypatch.setattr(product_read_authority,"read_document",point)
    result=catalog.get_table_usage("lake","silver","trips",Response(),SimpleNamespace(email="alice",role="Viewer"))
    assert result["datasets"][0]["id"]==13
    assert result["charts"][0]["id"]==21
    assert result["dashboards"][0]["slug"]=="mobility"
    assert result["dlm"][0]["datasetId"]==13
    retired.assert_not_called()


@pytest.mark.parametrize("operation,expected_pool_call",[
    (lambda: lab.list_tables(Response(),"warehouse","alice"),"get_tables"),
    (lambda: lab.get_table_columns("dbo.orders","warehouse","alice"),"get_table_columns"),
    (lambda: lab.get_schema("dbo","orders","warehouse","alice"),"get_table_columns"),
])
def test_lab_reaches_the_pool_for_a_registered_source_without_postgresql(
        monkeypatch,retired,operation,expected_pool_call):
    """These answered 503. The refusal was not the point -- `retired` is, and it
    fails if anything reaches `database.metadata`. The source resolver can read
    the registry from KaveonDB now, so the endpoint gets as far as the pool, and
    still touches no relational metadata doing it."""
    reached={}
    def record(name):
        def call(*args,**kwargs):
            reached["name"]=name
            reached["actor"]=kwargs.get("actor") or (args[1] if name=="get_tables" else args[2])
            return [] if name!="execute_query" else {"columns":[],"rows":[],"row_count":0}
        return call
    for name in ("get_tables","get_table_columns","execute_query"):
        monkeypatch.setattr(lab.pool,name,record(name))
    operation()
    assert reached["name"]==expected_pool_call
    assert reached["actor"]=="alice", "the pool needs the caller to read the registry as"
    retired.assert_not_called()


def test_execute_routes_an_engine_catalog_and_pools_everything_else(monkeypatch,retired):
    """`/sql/execute` refused any database that was not an Engine catalog. A
    registered source is not one and does not need to be: it goes to the pool,
    which resolves it from KaveonDB. Neither path touches PostgreSQL."""
    monkeypatch.setattr(product_read_authority,"list_documents",documents)
    assert sql._engine_source_for_catalog("lake")["engine_catalog"]=="lake"

    pooled={}
    def execute_query(sql_text,database,*args,**kwargs):
        pooled["database"]=database
        pooled["actor"]=kwargs.get("actor")
        return {"columns":[],"rows":[],"row_count":0}
    monkeypatch.setattr(sql.pool,"execute_query",execute_query)
    monkeypatch.setattr(sql.history_svc,"create_history",Mock())
    monkeypatch.setattr(sql.sql_execute_limiter,"check",Mock())
    monkeypatch.setattr(sql,"assert_no_platform_tables",Mock())

    context=SimpleNamespace(email="alice",role="Analyst")
    body=SimpleNamespace(sql_text="SELECT 1",database="warehouse",source="lab",
                         tables_used=None,chart_id=None,dashboard_id=None,
                         chart_type=None,dataset_id=None,row_limit=None,
                         use_cache=False,cache_ttl=None)
    sql.execute_sql(body,Response(),context)
    assert pooled["database"]=="warehouse"
    assert pooled["actor"]=="alice"
    retired.assert_not_called()
