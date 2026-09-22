import unittest
from unittest.mock import patch

import httpx
from fastapi import HTTPException, Response

from middleware.auth import UserContext
from routers import engine_catalog_access, lab
from services import engine_bridge

ADMIN = UserContext("admin@example.com", "Admin")
DOCUMENT = {
    "store": {"enabled": True, "generation": 4, "snapshot_id": "snapshot-4"},
    "catalogs": ["Kaveon", "OpenSource"],
    "reserved": [{"name": "KaveonDB", "grantable": False, "visible_to": "none",
                  "reason": "its read-only views are not available yet; hidden from every role until they are"}],
    "roles": {"reader": "browse", "analyst": "manage", "admin": "all"},
    "grants": [{"principal": "ana@example.com", "catalog": "OpenSource", "access": "query", "revision": 1,
                "granted_by": "admin@example.com", "granted_at_ms": 1}],
}


def _response(status, body=None):
    return httpx.Response(status, json=body, request=httpx.Request("GET", "http://engine/x"))


class CatalogAccessRouterTests(unittest.TestCase):
    def test_every_route_is_admin_only_and_never_reaches_the_engine_otherwise(self):
        body = engine_catalog_access.GrantBody(principal="ana@example.com", catalog="OpenSource", access="query")
        revoke = engine_catalog_access.RevokeBody(principal="ana@example.com", catalog="OpenSource", revision=1)
        for role in ("Viewer", "Analyst", "Editor"):
            ctx = UserContext("someone@example.com", role)
            for call in (
                lambda: engine_catalog_access.catalog_access(ctx),
                lambda: engine_catalog_access.grant_catalog_access(body, ctx),
                lambda: engine_catalog_access.revoke_catalog_access(revoke, ctx),
                lambda: engine_catalog_access.effective_catalog_access("ana@example.com", ctx),
                lambda: engine_catalog_access.import_catalog_access(engine_catalog_access.ImportBody(apply=True), ctx),
            ):
                with patch.object(engine_bridge, "_send") as send:
                    with self.assertRaises(HTTPException) as error:
                        call()
                self.assertEqual(error.exception.status_code, 403, role)
                send.assert_not_called()

    def test_the_document_is_read_through_the_bridge_as_the_verified_admin(self):
        with patch.object(engine_bridge, "_send", return_value=_response(200, DOCUMENT)) as send:
            self.assertEqual(engine_catalog_access.catalog_access(ADMIN), DOCUMENT)
        method, path, token_name, actor = send.call_args.args
        self.assertEqual((method, path, token_name, actor), ("GET", "/v1/admin/catalog-access", "KAVEON_ENGINE_BRIDGE_TOKEN", "admin@example.com"))
        self.assertEqual(send.call_args.kwargs["role"], "admin")

    def test_grants_forward_the_body_without_an_actor_and_keep_engine_conflicts(self):
        granted = {"grant": {**DOCUMENT["grants"][0], "access": "manage", "revision": 2}, "revision_before": 1, "generation": 5,
                   "effective": {"reader": "browse", "analyst": "manage", "admin": "all"}}
        body = engine_catalog_access.GrantBody(principal="ana@example.com", catalog="OpenSource", access="manage", revision=1)
        with patch.object(engine_bridge, "_send", return_value=_response(200, granted)) as send:
            self.assertEqual(engine_catalog_access.grant_catalog_access(body, ADMIN), granted)
        self.assertEqual(send.call_args.args[:2], ("PUT", "/v1/admin/catalog-access/grants"))
        self.assertEqual(send.call_args.kwargs["payload"], {"principal": "ana@example.com", "catalog": "OpenSource", "access": "manage", "revision": 1})
        self.assertEqual(send.call_args.kwargs["role"], "admin")
        # A new grant sends no revision at all.
        body = engine_catalog_access.GrantBody(principal="bob@example.com", catalog="Kaveon", access="browse")
        with patch.object(engine_bridge, "_send", return_value=_response(200, granted)) as send:
            engine_catalog_access.grant_catalog_access(body, ADMIN)
        self.assertNotIn("revision", send.call_args.kwargs["payload"])
        # A stale revision is the Engine's 409, message and code intact, so
        # the Studio reloads rather than retrying blindly.
        conflict = _response(409, {"error": "the grant for ana@example.com on OpenSource is at revision 2, not 1; reload", "code": "REVISION_CONFLICT"})
        with patch.object(engine_bridge, "_send", return_value=conflict):
            with self.assertRaises(HTTPException) as error:
                engine_catalog_access.grant_catalog_access(body, ADMIN)
        self.assertEqual(error.exception.status_code, 409)
        self.assertEqual(error.exception.detail["code"], "REVISION_CONFLICT")
        self.assertIn("not 1", error.exception.detail["message"])
        # The reserved authority and an unknown catalog keep their answers.
        reserved = _response(400, {"error": "KaveonDB is the transactional authority: administrators only, never granted", "code": "RESERVED_CATALOG"})
        with patch.object(engine_bridge, "_send", return_value=reserved):
            with self.assertRaises(HTTPException) as error:
                engine_catalog_access.grant_catalog_access(engine_catalog_access.GrantBody(principal="x@example.com", catalog="KaveonDB", access="browse"), ADMIN)
        self.assertEqual(error.exception.status_code, 422)
        self.assertEqual(error.exception.detail["code"], "RESERVED_CATALOG")
        with patch.object(engine_bridge, "_send", return_value=_response(404, {"error": "catalog 'nowhere' not found", "code": "CATALOG_NOT_FOUND"})):
            with self.assertRaises(HTTPException) as error:
                engine_catalog_access.grant_catalog_access(engine_catalog_access.GrantBody(principal="x@example.com", catalog="nowhere", access="browse"), ADMIN)
        self.assertEqual(error.exception.status_code, 404)
        # A store that is not configured is 503 with the Engine's reason.
        disabled = _response(503, {"error": "catalog access grants need the KaveonDB transaction store, which is not configured on this Engine", "code": "ACCESS_STORE_DISABLED"})
        with patch.object(engine_bridge, "_send", return_value=disabled):
            with self.assertRaises(HTTPException) as error:
                engine_catalog_access.grant_catalog_access(body, ADMIN)
        self.assertEqual(error.exception.status_code, 503)
        self.assertEqual(error.exception.detail["code"], "ACCESS_STORE_DISABLED")
        # Validation happens here before the Engine sees the request.
        for bad in (
            engine_catalog_access.GrantBody(principal="ana@example.com", catalog="OpenSource", access="owner"),
            engine_catalog_access.GrantBody(principal="ana example", catalog="OpenSource", access="query"),
            engine_catalog_access.GrantBody(principal="ana@example.com", catalog=" OpenSource", access="query"),
        ):
            with patch.object(engine_bridge, "_send") as send:
                with self.assertRaises(HTTPException) as error:
                    engine_catalog_access.grant_catalog_access(bad, ADMIN)
            self.assertEqual(error.exception.status_code, 422)
            send.assert_not_called()

    def test_revoke_and_effective_and_import_use_their_routes(self):
        revoked = {"revoked": DOCUMENT["grants"][0], "generation": 5}
        with patch.object(engine_bridge, "_send", return_value=_response(200, revoked)) as send:
            self.assertEqual(engine_catalog_access.revoke_catalog_access(
                engine_catalog_access.RevokeBody(principal="ana@example.com", catalog="OpenSource", revision=1), ADMIN), revoked)
        self.assertEqual(send.call_args.args[:2], ("DELETE", "/v1/admin/catalog-access/grants"))
        self.assertEqual(send.call_args.kwargs["payload"], {"principal": "ana@example.com", "catalog": "OpenSource", "revision": 1})
        effective = {"principal": "ana@example.com", "store_enabled": True, "grants": [], "ungranted": ["Kaveon", "OpenSource"], "reserved": [], "roles": {}}
        with patch.object(engine_bridge, "_send", return_value=_response(200, effective)) as send:
            self.assertEqual(engine_catalog_access.effective_catalog_access("ana@example.com", ADMIN), effective)
        self.assertEqual(send.call_args.args[1], "/v1/admin/catalog-access/effective/ana%40example.com")
        proposal = {"source": "open", "ledger_enabled": True, "principals_seen": 1, "catalogs": ["Kaveon", "OpenSource"],
                    "proposed": [{"principal": "ana@example.com", "role_seen": "analyst", "catalog": "Kaveon", "access": "manage"}],
                    "applied": False, "recorded": []}
        with patch.object(engine_bridge, "_send", return_value=_response(200, proposal)) as send:
            self.assertEqual(engine_catalog_access.import_catalog_access(engine_catalog_access.ImportBody(), ADMIN), proposal)
        self.assertEqual(send.call_args.args[:2], ("POST", "/v1/admin/catalog-access/import"))
        self.assertEqual(send.call_args.kwargs["payload"], {"source": "open", "apply": False})
        with patch.object(engine_bridge, "_send", return_value=_response(200, {**proposal, "applied": True})) as send:
            engine_catalog_access.import_catalog_access(engine_catalog_access.ImportBody(apply=True), ADMIN)
        self.assertEqual(send.call_args.kwargs["payload"], {"source": "open", "apply": True})
        with patch.object(engine_bridge, "_send") as send:
            with self.assertRaises(HTTPException) as error:
                engine_catalog_access.import_catalog_access(engine_catalog_access.ImportBody(source="everything"), ADMIN)
        self.assertEqual(error.exception.status_code, 422)
        send.assert_not_called()

    def test_my_access_is_read_for_any_role_as_the_verified_principal(self):
        mine = {"principal": "viewer@example.com", "role": "reader", "store_enabled": True,
                "catalogs": [{"catalog": "OpenSource", "access": "browse"}]}
        with patch.object(engine_bridge, "_send", return_value=_response(200, mine)) as send:
            self.assertEqual(engine_catalog_access.my_catalog_access(UserContext("viewer@example.com", "Viewer")), mine)
        self.assertEqual(send.call_args.args[:2], ("GET", "/v1/catalog-access/me"))
        self.assertEqual(send.call_args.args[3], "viewer@example.com")
        self.assertEqual(send.call_args.kwargs["role"], "reader")


class SqlLabSourcesTests(unittest.TestCase):
    def test_the_picker_lists_only_the_catalogs_the_engine_grants_the_principal(self):
        """The registry names every active native source; the Engine's
        catalog list for the verified principal decides which appear. The
        principal and role come from the proxy context, not from the request."""
        rows = [
            {"id": "source-1", "name": "Open data", "engine_catalog": "OpenSource"},
            {"id": "source-2", "name": "Kaveon", "engine_catalog": "Kaveon"},
        ]
        ctx = UserContext("ana@example.com", "Analyst")
        with patch.object(lab.meta_db, "query", return_value={"rows": rows}), \
             patch.object(engine_bridge, "_send", return_value=_response(200, {"catalogs": ["OpenSource"]})) as send:
            result = lab.list_engine_sources(Response(), ctx)
        self.assertEqual(result["sources"], [{"id": "source-1", "name": "Open data", "catalog": "OpenSource"}])
        self.assertEqual(send.call_args.args[:2], ("GET", "/v1/catalog"))
        self.assertEqual(send.call_args.args[3], "ana@example.com")
        self.assertEqual(send.call_args.kwargs["role"], "analyst")
        # Nothing granted: nothing listed, though the registry has sources.
        with patch.object(lab.meta_db, "query", return_value={"rows": rows}), \
             patch.object(engine_bridge, "_send", return_value=_response(200, {"catalogs": []})):
            self.assertEqual(lab.list_engine_sources(Response(), ctx)["sources"], [])
        # An Engine that cannot answer fails the list rather than showing
        # the registry unfiltered.
        with patch.object(lab.meta_db, "query", return_value={"rows": rows}), \
             patch.object(engine_bridge, "_send", return_value=_response(503, {"error": "starting"})):
            with self.assertRaises(HTTPException) as error:
                lab.list_engine_sources(Response(), ctx)
        self.assertEqual(error.exception.status_code, 502)
