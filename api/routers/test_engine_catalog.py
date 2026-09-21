import unittest
from unittest.mock import patch

from fastapi import HTTPException, Response
from pydantic import ValidationError

from middleware.auth import UserContext
from models.engine_catalog import ColumnSpec, TableCreate, arrow_type
from routers import engine_catalog
from services import engine_bridge

CATALOG = {"id": "aks-benchmarks", "name": "Benchmarks", "revision": 2, "adapter": "Native",
           "storage": {"AdlsGen2": {"account": "acct", "container": "opensource", "root_path": "benchmarks"}},
           "credential": {"kind": "WorkloadIdentity", "reference": "kaveon-test-reader"}, "lifecycle": "Active"}
SCHEMA = {"id": "aks-benchmarks-tpch_sf1", "catalog_id": "aks-benchmarks", "name": "tpch_sf1", "revision": 2, "lifecycle": "Active"}
TABLE_BODY = {"schema_id": "aks-benchmarks-tpch_sf1", "name": "region", "location": "tpch/sf1/region",
              "format": "delta", "columns": [{"name": "r_regionkey", "type": "bigint", "nullable": False},
                                             {"name": "r_name", "type": "varchar(25)"}]}
EDITOR = UserContext("editor@example.com", "Editor")


class TypeMappingTests(unittest.TestCase):
    def test_trino_names_map_to_the_engine_arrow_types_the_scripts_use(self):
        self.assertEqual(arrow_type("bigint"), "Int64")
        self.assertEqual(arrow_type("integer"), "Int32")
        self.assertEqual(arrow_type("smallint"), "Int16")
        self.assertEqual(arrow_type("double"), "Float64")
        self.assertEqual(arrow_type("varchar(25)"), "Utf8")
        self.assertEqual(arrow_type("DATE"), "Date32")
        self.assertEqual(arrow_type("timestamp"), {"Timestamp": ["Microsecond", None]})
        self.assertEqual(arrow_type("timestamp(3)"), {"Timestamp": ["Millisecond", None]})
        self.assertEqual(arrow_type("decimal(18, 2)"), {"Decimal128": [18, 2]})
        self.assertEqual(arrow_type("Int64"), "Int64")
        self.assertEqual(arrow_type({"Timestamp": ["Nanosecond", "UTC"]}), {"Timestamp": ["Nanosecond", "UTC"]})
        for bad in ("json", "decimal(40, 2)", "timestamp(2)", {"A": 1, "B": 2}):
            with self.assertRaises(ValueError):
                arrow_type(bad)

    def test_column_and_table_bodies_validate_before_the_engine_sees_them(self):
        self.assertEqual(ColumnSpec(name="x", type="bigint").engine(), {"name": "x", "data_type": "Int64", "nullable": True})
        body = TableCreate(**TABLE_BODY)
        self.assertEqual((body.format, body.access, body.verify), ("Delta", "Shortcut", True))
        with self.assertRaises(ValidationError):
            TableCreate(**{**TABLE_BODY, "location": "abfss://c@a.dfs.core.windows.net/x"})
        with self.assertRaises(ValidationError):
            TableCreate(**{**TABLE_BODY, "name": "bad name"})
        # No columns is a valid body: the Engine infers them from the table.
        self.assertEqual(TableCreate(**{**TABLE_BODY, "columns": []}).columns, [])
        self.assertEqual(TableCreate(**{k: v for k, v in TABLE_BODY.items() if k != "columns"}).columns, [])
        with self.assertRaises(ValidationError):
            TableCreate(**{**TABLE_BODY, "columns": [{"name": "a", "type": "bigint"}, {"name": "a", "type": "bigint"}]})
        with self.assertRaises(ValidationError):
            TableCreate(**{**TABLE_BODY, "format": "orc"})


class TableRegistrationTests(unittest.TestCase):
    def test_table_is_created_activated_verified_and_reported(self):
        active = {"id": "aks-benchmarks-tpch_sf1-region", "schema_id": SCHEMA["id"], "name": "region", "revision": 2,
                  "location": "tpch/sf1/region", "access": "Shortcut", "format": "Delta", "lifecycle": "Active",
                  "columns": [{"name": "r_regionkey", "data_type": "Int64", "nullable": False}]}
        with patch.object(engine_bridge, "schema_definition", return_value=SCHEMA), \
             patch.object(engine_bridge, "catalog_definition", return_value=CATALOG), \
             patch.object(engine_bridge, "create_table", return_value=active) as create, \
             patch.object(engine_bridge, "probe_table", return_value={"ok": True, "row_count": 5, "elapsed_ms": 12, "query_id": "q1"}) as probe, \
             patch.object(engine_bridge, "delete_table") as delete:
            result = engine_catalog.create_table_definition(TableCreate(**TABLE_BODY), EDITOR)
        self.assertEqual(result["table"], active)
        self.assertEqual(result["probe"], {"rowCount": 5, "elapsedMs": 12, "queryId": "q1"})
        definition, actor = create.call_args.args
        self.assertEqual(actor, "editor@example.com")
        self.assertEqual(definition["id"], "aks-benchmarks-tpch_sf1-region")
        self.assertEqual(definition["format"], "Delta")
        self.assertEqual(definition["columns"], [
            {"name": "r_regionkey", "data_type": "Int64", "nullable": False},
            {"name": "r_name", "data_type": "Utf8", "nullable": True},
        ])
        probe.assert_called_once_with("Benchmarks", "tpch_sf1", "region", "editor@example.com", "Editor")
        delete.assert_not_called()

    def test_a_table_without_columns_is_registered_by_the_engine_statement_with_inferred_columns(self):
        inferred = {"id": "table:Benchmarks:tpch_sf1:region", "schema_id": SCHEMA["id"], "name": "region",
                    "revision": 2, "lifecycle": "Active", "format": "Delta",
                    "columns": [{"name": "r_regionkey", "data_type": "Int64", "nullable": True}]}
        with patch.object(engine_bridge, "schema_definition", return_value=SCHEMA),              patch.object(engine_bridge, "catalog_definition", return_value=CATALOG),              patch.object(engine_bridge, "create_table_inferred", return_value={"ok": True, "result": {}}) as create,              patch.object(engine_bridge, "table_definitions", return_value=[inferred]),              patch.object(engine_bridge, "probe_table", return_value={"ok": True, "row_count": 5, "elapsed_ms": 9, "query_id": "q2"}),              patch.object(engine_bridge, "create_table") as create_with_columns:
            body = TableCreate(**{k: v for k, v in TABLE_BODY.items() if k != "columns"})
            result = engine_catalog.create_table_definition(body, EDITOR)
        self.assertEqual(result["table"], inferred)
        self.assertEqual(result["probe"], {"rowCount": 5, "elapsedMs": 9, "queryId": "q2"})
        create.assert_called_once_with("Benchmarks", "tpch_sf1", "region", TABLE_BODY["location"], "Delta",
                                       "editor@example.com", "Editor")
        create_with_columns.assert_not_called()

    def test_an_inferred_registration_the_engine_refuses_registers_nothing(self):
        message = "storage: object not found: benchmarks/tpch/sf1/region/_delta_log"
        with patch.object(engine_bridge, "schema_definition", return_value=SCHEMA),              patch.object(engine_bridge, "catalog_definition", return_value=CATALOG),              patch.object(engine_bridge, "create_table_inferred",
                          return_value={"ok": False, "message": message, "code": "TABLE_NOT_READABLE"}),              patch.object(engine_bridge, "delete_table") as delete:
            with self.assertRaises(HTTPException) as error:
                engine_catalog.create_table_definition(
                    TableCreate(**{k: v for k, v in TABLE_BODY.items() if k != "columns"}), EDITOR)
        detail = error.exception.detail
        self.assertEqual((error.exception.status_code, detail["code"], detail["message"], detail["removed"]),
                         (422, "table_unreadable", message, True))
        delete.assert_not_called()

    def test_unreadable_table_is_removed_and_the_storage_error_returned_verbatim(self):
        active = {"id": "aks-benchmarks-tpch_sf1-region", "revision": 2}
        message = "execution error: object not found: benchmarks/tpch/sf1/region/_delta_log"
        with patch.object(engine_bridge, "schema_definition", return_value=SCHEMA), \
             patch.object(engine_bridge, "catalog_definition", return_value=CATALOG), \
             patch.object(engine_bridge, "create_table", return_value=active), \
             patch.object(engine_bridge, "probe_table", return_value={"ok": False, "message": message, "code": "EXECUTION_ERROR"}), \
             patch.object(engine_bridge, "delete_table") as delete:
            with self.assertRaises(HTTPException) as error:
                engine_catalog.create_table_definition(TableCreate(**TABLE_BODY), EDITOR)
        self.assertEqual(error.exception.status_code, 422)
        detail = error.exception.detail
        self.assertEqual((detail["code"], detail["message"], detail["engineCode"], detail["removed"]),
                         ("table_unreadable", message, "EXECUTION_ERROR", True))
        delete.assert_called_once_with("aks-benchmarks-tpch_sf1-region", 2, "editor@example.com")

    def test_verification_requires_active_parents_and_skips_when_declined(self):
        draft_schema = {**SCHEMA, "lifecycle": "Draft"}
        with patch.object(engine_bridge, "schema_definition", return_value=draft_schema), \
             patch.object(engine_bridge, "catalog_definition", return_value=CATALOG), \
             patch.object(engine_bridge, "create_table") as create:
            with self.assertRaises(HTTPException) as error:
                engine_catalog.create_table_definition(TableCreate(**TABLE_BODY), EDITOR)
            self.assertEqual(error.exception.status_code, 409)
            create.assert_not_called()
            create.return_value = {"id": "t", "revision": 2}
            with patch.object(engine_bridge, "probe_table") as probe:
                result = engine_catalog.create_table_definition(TableCreate(**TABLE_BODY, verify=False), EDITOR)
            probe.assert_not_called()
        self.assertIsNone(result["probe"])

    def test_missing_schema_is_404_before_anything_is_created(self):
        with patch.object(engine_bridge, "schema_definition", return_value=None), \
             patch.object(engine_bridge, "create_table") as create:
            with self.assertRaises(HTTPException) as error:
                engine_catalog.create_table_definition(TableCreate(**TABLE_BODY), EDITOR)
        self.assertEqual(error.exception.status_code, 404)
        create.assert_not_called()

    def test_delete_and_replace_need_the_current_revision(self):
        with self.assertRaises(HTTPException) as error:
            engine_catalog.delete_table_definition("t1", None, EDITOR)
        self.assertEqual(error.exception.status_code, 428)
        with patch.object(engine_bridge, "table_definition_by_id", return_value={"id": "t1", "revision": 3}), \
             patch.object(engine_bridge, "delete_table") as delete:
            response = engine_catalog.delete_table_definition("t1", '"3"', EDITOR)
        self.assertEqual(response.status_code, 204)
        delete.assert_called_once_with("t1", 3, "editor@example.com")
        with patch.object(engine_bridge, "table_definition_by_id", return_value=None):
            with self.assertRaises(HTTPException) as error:
                engine_catalog.delete_table_definition("gone", "3", EDITOR)
        self.assertEqual(error.exception.status_code, 404)


class SchemaRegistrationTests(unittest.TestCase):
    def test_schema_id_derives_from_the_catalog_id_and_needs_an_active_catalog(self):
        from models.engine_catalog import SchemaCreate
        with patch.object(engine_bridge, "catalog_definition", return_value=CATALOG), \
             patch.object(engine_bridge, "create_schema", return_value={**SCHEMA, "name": "silver"}) as create:
            result = engine_catalog.create_schema_definition("aks-benchmarks", SchemaCreate(name="silver"), EDITOR)
        self.assertEqual(result["schema"]["name"], "silver")
        create.assert_called_once_with("aks-benchmarks", "aks-benchmarks-silver", "silver", "editor@example.com")
        with patch.object(engine_bridge, "catalog_definition", return_value={**CATALOG, "lifecycle": "Draft"}), \
             patch.object(engine_bridge, "create_schema") as create:
            with self.assertRaises(HTTPException) as error:
                engine_catalog.create_schema_definition("aks-benchmarks", SchemaCreate(name="silver"), EDITOR)
        self.assertEqual(error.exception.status_code, 409)
        create.assert_not_called()

    def test_listing_uses_the_bridge_read_role(self):
        ctx = UserContext("viewer@example.com", "Viewer")
        with patch.object(engine_bridge, "_request", return_value=[CATALOG]) as request:
            result = engine_catalog.list_catalog_definitions(Response(), ctx)
        self.assertEqual(result["definitions"], [CATALOG])
        self.assertEqual(request.call_args.args[1], "/v1/catalog/definitions")
        self.assertEqual(request.call_args.kwargs["role"], "reader")


class BridgeTests(unittest.TestCase):
    class _Response:
        def __init__(self, status, body=None):
            self.status_code, self._body = status, body
            self.is_success = 200 <= status < 300

        def json(self):
            if self._body is None:
                raise ValueError("no body")
            return self._body

    def test_create_table_posts_draft_then_activates_with_if_match(self):
        calls = []

        def send(method, path, token_name, actor, *, payload=None, revision=None, role=None, timeout=60):
            calls.append((method, path, token_name, payload, revision))
            if method == "POST":
                return self._Response(201, {**payload})
            return self._Response(200, {**payload})

        definition = {"id": "s-t", "schema_id": "s", "name": "t", "location": "t", "access": "Shortcut",
                      "format": "Parquet", "columns": [{"name": "a", "data_type": "Int64", "nullable": True}]}
        with patch.object(engine_bridge, "_send", side_effect=send):
            active = engine_bridge.create_table(definition, "editor@example.com")
        self.assertEqual((active["revision"], active["lifecycle"]), (2, "Active"))
        self.assertEqual(calls[0][:3], ("POST", "/v1/catalog/schemas/s/tables", "KAVEON_ENGINE_CATALOG_TOKEN"))
        self.assertEqual((calls[0][3]["revision"], calls[0][3]["lifecycle"]), (1, "Draft"))
        self.assertEqual(calls[1][:2], ("PUT", "/v1/catalog/tables/s-t"))
        self.assertEqual(calls[1][4], 1)

    def test_engine_refusals_keep_their_message(self):
        with patch.object(engine_bridge, "_send", return_value=self._Response(400, {"error": "table column names must be unique"})):
            with self.assertRaises(HTTPException) as error:
                engine_bridge.catalog_request("POST", "/v1/catalog/schemas/s/tables", "a", payload={})
        self.assertEqual((error.exception.status_code, error.exception.detail), (422, "table column names must be unique"))
        with patch.object(engine_bridge, "_send", return_value=self._Response(409, {"error": "schema 's' contains tables"})):
            with self.assertRaises(HTTPException) as error:
                engine_bridge.catalog_request("DELETE", "/v1/catalog/schemas/s", "a", revision=2)
        self.assertEqual((error.exception.status_code, error.exception.detail), (409, "schema 's' contains tables"))

    def test_probe_reports_count_or_the_engine_error_without_raising(self):
        with patch.object(engine_bridge, "_send", return_value=self._Response(200, {"id": "q1", "data": [[42]], "elapsed_ms": 7})) as send:
            probe = engine_bridge.probe_table("Benchmarks", "tpch_sf1", "region", "editor@example.com", "Editor")
        self.assertEqual(probe, {"ok": True, "row_count": 42, "elapsed_ms": 7, "query_id": "q1"})
        payload = send.call_args.kwargs["payload"]
        self.assertEqual(payload["query"], "SELECT COUNT(*) FROM tpch_sf1.region")
        self.assertEqual(payload["settings"], {"result_cache": False})
        self.assertEqual(send.call_args.kwargs["role"], "analyst")
        with patch.object(engine_bridge, "_send", return_value=self._Response(500, {"error": "execution error: not found", "code": "EXECUTION_ERROR"})):
            probe = engine_bridge.probe_table("Benchmarks", "tpch_sf1", "region", "editor@example.com", "Editor")
        self.assertEqual(probe, {"ok": False, "message": "execution error: not found", "code": "EXECUTION_ERROR"})
        with self.assertRaises(HTTPException) as error:
            engine_bridge.probe_table("Benchmarks", "tpch_sf1", "region", "viewer@example.com", "Viewer")
        self.assertEqual(error.exception.status_code, 403)


if __name__ == "__main__":
    unittest.main()
