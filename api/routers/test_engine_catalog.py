import unittest
from unittest.mock import patch

from fastapi import HTTPException, Response
from pydantic import ValidationError

from middleware.auth import UserContext
from models.engine_catalog import ColumnSpec, TableAnalyze, TableCreate, TableReplace, arrow_type
from routers import engine_catalog
from services import engine_bridge

# The Engine annotates a definition it returns for a principal with that
# principal's level on the catalog; the Editor here holds `manage`.
CATALOG = {"id": "aks-benchmarks", "name": "Benchmarks", "revision": 2, "adapter": "Native",
           "storage": {"AdlsGen2": {"account": "acct", "container": "opensource", "root_path": "benchmarks"}},
           "credential": {"kind": "WorkloadIdentity", "reference": "kaveon-test-reader"}, "lifecycle": "Active",
           "access": "manage"}
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
        with patch.object(engine_bridge, "table_definition_by_id", return_value={"id": "t1", "revision": 3, "schema_id": SCHEMA["id"]}), \
             patch.object(engine_bridge, "schema_definition", return_value=SCHEMA), \
             patch.object(engine_bridge, "catalog_definition", return_value=CATALOG), \
             patch.object(engine_bridge, "delete_table") as delete:
            response = engine_catalog.delete_table_definition("t1", '"3"', EDITOR)
        self.assertEqual(response.status_code, 204)
        delete.assert_called_once_with("t1", 3, "editor@example.com")
        with patch.object(engine_bridge, "table_definition_by_id", return_value=None):
            with self.assertRaises(HTTPException) as error:
                engine_catalog.delete_table_definition("gone", "3", EDITOR)
        self.assertEqual(error.exception.status_code, 404)

    def test_changes_inside_a_catalog_need_the_manage_level_the_engine_reports(self):
        """An Editor granted `query` (or nothing the Engine reported) on the
        catalog can read its definitions but not change them: every
        registration route checks the level before anything is created."""
        from models.engine_catalog import SchemaCreate
        table = {"id": "t1", "revision": 3, "schema_id": SCHEMA["id"], "name": "region", "location": "x",
                 "access": "Shortcut", "format": "Delta", "lifecycle": "Active", "columns": []}
        for catalog in ({**CATALOG, "access": "query"}, {**CATALOG, "access": "browse"}, {key: value for key, value in CATALOG.items() if key != "access"}):
            with patch.object(engine_bridge, "catalog_definition", return_value=catalog), \
                 patch.object(engine_bridge, "schema_definition", return_value=SCHEMA), \
                 patch.object(engine_bridge, "table_definition_by_id", return_value=table), \
                 patch.object(engine_bridge, "create_schema") as create_schema, \
                 patch.object(engine_bridge, "create_table") as create_table, \
                 patch.object(engine_bridge, "create_table_inferred") as create_inferred, \
                 patch.object(engine_bridge, "replace_table") as replace, \
                 patch.object(engine_bridge, "delete_schema") as delete_schema, \
                 patch.object(engine_bridge, "delete_table") as delete_table:
                for call in (
                    lambda: engine_catalog.create_schema_definition("aks-benchmarks", SchemaCreate(name="silver"), EDITOR),
                    lambda: engine_catalog.delete_schema_definition(SCHEMA["id"], "2", EDITOR),
                    lambda: engine_catalog.create_table_definition(TableCreate(**TABLE_BODY), EDITOR),
                    lambda: engine_catalog.create_table_definition(TableCreate(**{**TABLE_BODY, "columns": []}), EDITOR),
                    lambda: engine_catalog.replace_table_definition("t1", TableReplace(**{key: TABLE_BODY[key] for key in ("name", "location", "format", "columns")}), "3", EDITOR),
                    lambda: engine_catalog.delete_table_definition("t1", "3", EDITOR),
                ):
                    with self.assertRaises(HTTPException) as error:
                        call()
                    self.assertEqual(error.exception.status_code, 403, catalog.get("access"))
                    self.assertEqual(error.exception.detail["code"], "catalog_access")
                for mutation in (create_schema, create_table, create_inferred, replace, delete_schema, delete_table):
                    mutation.assert_not_called()
        # A catalog the Engine hid from the principal is not found at all.
        with patch.object(engine_bridge, "catalog_definition", return_value=None), \
             patch.object(engine_bridge, "create_schema") as create_schema:
            with self.assertRaises(HTTPException) as error:
                engine_catalog.create_schema_definition("aks-benchmarks", SchemaCreate(name="silver"), EDITOR)
        self.assertEqual(error.exception.status_code, 404)
        create_schema.assert_not_called()


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


class InventoryTests(unittest.TestCase):
    """The per-schema batch: one call, one row per table, and never a whole
    page lost to the one table the Engine cannot read."""

    VIEWER = UserContext("viewer@example.com", "Viewer")

    def setUp(self):
        engine_catalog._INVENTORY_CACHE.clear()

    @staticmethod
    def _table(name):
        return {"id": f"table:Benchmarks:tpch_sf1:{name}", "schema_id": SCHEMA["id"], "name": name,
                "location": f"tpch/sf1/{name}", "access": "Shortcut", "format": "Delta",
                "revision": 2, "lifecycle": "Active", "columns": []}

    STATISTICS = {
        "state": "measured", "table_id": "table:Benchmarks:tpch_sf1:region",
        "source_version": {"kind": "delta_version", "version": 4, "identity_sha256": "a" * 64},
        "current_source_version": {"kind": "delta_version", "version": 5, "identity_sha256": "b" * 64},
        "observed_at_ms": 1790000000000, "stale": True,
        "statistics": {"rows": 5, "bytes": 2284, "files": 1, "row_groups": 1, "uncompressed_bytes": 1424,
                       "computed_at_ms": 1789000000000, "depth": "metadata", "partition_columns": ["r_year"],
                       "source_version": {"kind": "delta_version", "version": 4, "identity_sha256": "a" * 64},
                       "columns": [{"name": "r_regionkey"}, {"name": "r_name"}]},
    }

    def _inventory(self, measurements):
        tables = [self._table(name) for name in ("region", "nation", "orders")]
        with patch.object(engine_bridge, "schema_definition", return_value=SCHEMA), \
             patch.object(engine_bridge, "table_definitions", return_value=tables), \
             patch.object(engine_bridge, "table_measurement", side_effect=measurements):
            return engine_catalog.schema_inventory(SCHEMA["id"], Response(), False, self.VIEWER)["measurements"]

    def test_every_table_is_one_row_in_the_platform_shape(self):
        rows = self._inventory([
            self.STATISTICS,
            {"state": "unmeasured", "source_version": {"kind": "listing", "files": 12, "identity_sha256": "c" * 64},
             "observed_at_ms": 1790000000001},
            {"state": "unreadable", "error": "source version of Benchmarks.tpch_sf1.orders is unreadable: no such path"},
        ])
        self.assertEqual([row["state"] for row in rows], ["measured", "unmeasured", "unreadable"])
        measured = rows[0]
        self.assertEqual((measured["rows"], measured["bytes"], measured["files"]), (5, 2284, 1))
        self.assertEqual((measured["depth"], measured["stale"]), ("metadata", True))
        self.assertEqual(measured["partitionColumns"], ["r_year"])
        # Column-level facts stay on the table's own page; only the count travels.
        self.assertEqual(measured["measuredColumns"], 2)
        self.assertNotIn("columns", measured)
        self.assertEqual(rows[1]["sourceVersion"]["files"], 12)
        self.assertIsNone(rows[1].get("rows"))
        # The storage error is the Engine's own words, kept whole.
        self.assertIn("no such path", rows[2]["error"])

    def test_one_refused_table_does_not_lose_the_others(self):
        rows = self._inventory([HTTPException(502, "Engine is unavailable"), self.STATISTICS, self.STATISTICS])
        self.assertEqual(rows[0], {"tableId": "table:Benchmarks:tpch_sf1:region", "state": "unreadable",
                                   "error": "Engine is unavailable"})
        self.assertEqual([row["state"] for row in rows[1:]], ["measured", "measured"])

    def test_a_repeat_read_is_answered_from_the_last_one_until_refresh(self):
        tables = [self._table("region")]
        with patch.object(engine_bridge, "schema_definition", return_value=SCHEMA), \
             patch.object(engine_bridge, "table_definitions", return_value=tables), \
             patch.object(engine_bridge, "table_measurement", return_value=self.STATISTICS) as measure:
            engine_catalog.schema_inventory(SCHEMA["id"], Response(), False, self.VIEWER)
            engine_catalog.schema_inventory(SCHEMA["id"], Response(), False, self.VIEWER)
            self.assertEqual(measure.call_count, 1)
            engine_catalog.schema_inventory(SCHEMA["id"], Response(), True, self.VIEWER)
            self.assertEqual(measure.call_count, 2)

    def test_an_unknown_schema_is_not_found(self):
        with patch.object(engine_bridge, "schema_definition", return_value=None):
            with self.assertRaises(HTTPException) as error:
                engine_catalog.schema_inventory("nope", Response(), False, self.VIEWER)
        self.assertEqual(error.exception.status_code, 404)


class AnalyzeTests(unittest.TestCase):
    """ANALYZE is assembled from the table's own names, needs `manage`, and is
    never offered a cube over a table that declares no shape."""

    TABLE = {"id": "aks-benchmarks-tpch_sf1-region", "schema_id": SCHEMA["id"], "name": "region",
             "location": "tpch/sf1/region", "access": "Shortcut", "format": "Delta", "revision": 2,
             "lifecycle": "Active", "columns": []}

    def _run(self, body, table=None):
        with patch.object(engine_bridge, "table_definition_by_id", return_value=table or self.TABLE), \
             patch.object(engine_bridge, "schema_definition", return_value=SCHEMA), \
             patch.object(engine_bridge, "catalog_definition", return_value=CATALOG), \
             patch.object(engine_bridge, "analyze_table",
                          return_value=("ANALYZE tpch_sf1.region",
                                        {"id": "q9",
                                         "columns": [{"name": "table"}, {"name": "row_count"}, {"name": "cube_cells"}],
                                         "data": [["Benchmarks.tpch_sf1.region", 5, 240]]})) as analyze:
            result = engine_catalog.analyze_table_definition("aks-benchmarks-tpch_sf1-region", body, EDITOR)
        return result, analyze

    def test_the_depth_flags_reach_the_bridge_and_the_summary_comes_back(self):
        result, analyze = self._run(TableAnalyze(sketches=True))
        self.assertEqual(analyze.call_args.args[:3], ("Benchmarks", "tpch_sf1", "region"))
        self.assertEqual(analyze.call_args.kwargs, {"sketches": True, "distinct": False, "cube": False})
        self.assertEqual(result["result"]["row_count"], 5)
        self.assertEqual(result["result"]["cube_cells"], 240)
        self.assertEqual(result["queryId"], "q9")

    def test_a_cube_needs_a_declared_shape(self):
        with patch.object(engine_bridge, "table_definition_by_id", return_value=self.TABLE), \
             patch.object(engine_bridge, "schema_definition", return_value=SCHEMA), \
             patch.object(engine_bridge, "catalog_definition", return_value=CATALOG):
            with self.assertRaises(HTTPException) as error:
                engine_catalog.analyze_table_definition(
                    "aks-benchmarks-tpch_sf1-region", TableAnalyze(cube=True), EDITOR)
        self.assertEqual(error.exception.status_code, 409)
        self.assertEqual(error.exception.detail["code"], "no_shape")
        shaped = {**self.TABLE, "shape": {"dimensions": [{"name": "r_name"}]}}
        result, _ = self._run(TableAnalyze(cube=True), table=shaped)
        self.assertTrue(result["success"])

    def test_changing_statistics_needs_manage_on_the_catalog(self):
        with patch.object(engine_bridge, "table_definition_by_id", return_value=self.TABLE), \
             patch.object(engine_bridge, "schema_definition", return_value=SCHEMA), \
             patch.object(engine_bridge, "catalog_definition", return_value={**CATALOG, "access": "read"}):
            with self.assertRaises(HTTPException) as error:
                engine_catalog.analyze_table_definition("aks-benchmarks-tpch_sf1-region", TableAnalyze(), EDITOR)
        self.assertEqual(error.exception.status_code, 403)


class MeasurementBridgeTests(unittest.TestCase):
    """`table_measurement` separates the three outcomes without raising, and
    `analyze_table` builds its own statement and keeps the Engine's refusal."""

    class _Response:
        def __init__(self, status, body=None):
            self.status_code, self._body = status, body
            self.is_success = 200 <= status < 300

        def json(self):
            if self._body is None:
                raise ValueError("no body")
            return self._body

    def test_statistics_on_record_come_back_as_measured(self):
        body = {"table_id": "t", "stale": False, "statistics": {"rows": 7}}
        with patch.object(engine_bridge, "_send", return_value=self._Response(200, body)):
            result = engine_bridge.table_measurement("t", "a@b.c", "Viewer")
        self.assertEqual(result["state"], "measured")
        self.assertEqual(result["statistics"], {"rows": 7})

    def test_a_table_never_analyzed_still_reports_its_version(self):
        version = {"table_id": "t", "source_version": {"kind": "file", "identity_sha256": "d" * 64},
                   "observed_at_ms": 1}
        responses = [self._Response(404, {"code": "STATISTICS_UNAVAILABLE"}), self._Response(200, version)]
        with patch.object(engine_bridge, "_send", side_effect=responses):
            result = engine_bridge.table_measurement("t", "a@b.c", "Viewer")
        self.assertEqual(result["state"], "unmeasured")
        self.assertEqual(result["source_version"]["kind"], "file")

    def test_an_unreadable_location_keeps_the_engines_words(self):
        message = "source version of C.s.t is unreadable: failed to list /data/t"
        with patch.object(engine_bridge, "_send",
                          return_value=self._Response(502, {"error": message, "code": "SOURCE_UNAVAILABLE"})):
            result = engine_bridge.table_measurement("t", "a@b.c", "Viewer")
        self.assertEqual(result, {"state": "unreadable", "error": message})

    def test_analyze_assembles_the_statement_and_raises_the_engines_refusal(self):
        with patch.object(engine_bridge, "_send",
                          return_value=self._Response(200, {"id": "q", "data": [], "columns": []})) as send:
            statement, _ = engine_bridge.analyze_table("C", "s", "t", "a@b.c", "Editor",
                                                       sketches=True, distinct=True)
        self.assertEqual(statement, "ANALYZE s.t WITH (distinct = true, sketches = true)")
        self.assertEqual(send.call_args.kwargs["payload"]["query"], statement)
        self.assertEqual(engine_bridge._quote_ident("region"), "region")
        self.assertEqual(engine_bridge._quote_ident('a"b'), '"a""b"')
        with patch.object(engine_bridge, "_send",
                          return_value=self._Response(400, {"error": "C.s.t is not in the durable catalog",
                                                            "code": "TABLE_NOT_FOUND"})):
            with self.assertRaises(HTTPException) as error:
                engine_bridge.analyze_table("C", "s", "t", "a@b.c", "Editor")
        self.assertEqual(error.exception.status_code, 422)
        self.assertEqual(error.exception.detail["message"], "C.s.t is not in the durable catalog")
        self.assertEqual(error.exception.detail["engineCode"], "TABLE_NOT_FOUND")


if __name__ == "__main__":
    unittest.main()
