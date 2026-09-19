import asyncio
import unittest
from unittest.mock import patch

import httpx
from fastapi import HTTPException
from starlette.datastructures import QueryParams

from middleware.auth import UserContext
from routers import engine_governance
from services import engine_bridge


def _response(status, body=None, text=None):
    return httpx.Response(status, json=body, text=text, request=httpx.Request("GET", "http://engine/x"))


class _Request:
    def __init__(self, **params):
        self.query_params = QueryParams(params)


class EngineGovernanceTests(unittest.TestCase):
    def test_resource_groups_are_admin_only_and_read_through_the_bridge(self):
        admin = UserContext("admin@example.com", "Admin")
        document = {"source": "runtime", "groups": [{"name": "default", "max_concurrent": 4}], "selectors": [], "counters": []}
        with patch.object(engine_bridge, "_send", return_value=_response(200, document)) as send:
            self.assertEqual(engine_governance.resource_groups(admin), document)
        method, path, token_name, actor = send.call_args.args
        self.assertEqual((method, path, token_name, actor), ("GET", "/v1/admin/resource-groups", "KAVEON_ENGINE_BRIDGE_TOKEN", "admin@example.com"))
        self.assertEqual(send.call_args.kwargs["role"], "admin")
        for role in ("Viewer", "Analyst", "Editor"):
            with patch.object(engine_bridge, "_send") as send:
                with self.assertRaises(HTTPException) as error:
                    engine_governance.resource_groups(UserContext("someone@example.com", role))
            self.assertEqual(error.exception.status_code, 403)
            send.assert_not_called()

    def test_replacement_is_validated_here_and_engine_refusals_keep_their_message(self):
        admin = UserContext("admin@example.com", "Admin")
        document = {"groups": [{"name": "default", "max_concurrent": 2}], "selectors": [{"role": "analyst", "group": "default"}]}
        applied = {**document, "source": "runtime", "counters": []}
        with patch.object(engine_bridge, "_send", return_value=_response(200, applied)) as send:
            self.assertEqual(engine_governance.replace_resource_groups(document, admin), applied)
        self.assertEqual(send.call_args.args[:2], ("PUT", "/v1/admin/resource-groups"))
        self.assertEqual(send.call_args.kwargs["payload"], document)
        with self.assertRaises(HTTPException) as error:
            engine_governance.replace_resource_groups({"selectors": []}, admin)
        self.assertEqual(error.exception.status_code, 422)
        refusal = _response(400, {"error": "resource groups must include a 'default' group", "code": "INVALID_RESOURCE_GROUPS"})
        with patch.object(engine_bridge, "_send", return_value=refusal):
            with self.assertRaises(HTTPException) as error:
                engine_governance.replace_resource_groups({"groups": [{"name": "x", "max_concurrent": 1}]}, admin)
        self.assertEqual(error.exception.status_code, 422)
        self.assertIn("default", error.exception.detail)
        with patch.object(engine_bridge, "_send", return_value=_response(403, {"error": "resource groups require admin role"})):
            with self.assertRaises(HTTPException) as error:
                engine_governance.resource_groups(admin)
        self.assertEqual(error.exception.status_code, 403)

    def test_audit_passes_the_filters_through_and_pages(self):
        admin = UserContext("admin@example.com", "Admin")
        page = {"records": [{"seq": 5, "kind": "statement.finished"}], "next_cursor": 5}
        request = _Request(since="2026-09-01", principal="alice", kind="statement,auth", limit="50", cursor="4", ignored="x")
        with patch.object(engine_bridge, "_send", return_value=_response(200, page)) as send:
            self.assertEqual(engine_governance.audit(request, admin), page)
        path = send.call_args.args[1]
        self.assertTrue(path.startswith("/v1/audit?"), path)
        query = dict(part.split("=", 1) for part in path.split("?", 1)[1].split("&"))
        self.assertEqual(query, {"since": "2026-09-01", "principal": "alice", "kind": "statement%2Cauth", "limit": "50", "cursor": "4", "format": "json"})
        self.assertEqual(send.call_args.kwargs["role"], "admin")
        with patch.object(engine_bridge, "_send", return_value=_response(400, {"error": "since must be Unix milliseconds, YYYY-MM-DD or an RFC 3339 UTC timestamp", "code": "INVALID_AUDIT_QUERY"})):
            with self.assertRaises(HTTPException) as error:
                engine_governance.audit(_Request(since="yesterday"), admin)
        self.assertEqual(error.exception.status_code, 422)
        self.assertIn("since", error.exception.detail)
        with patch.object(engine_bridge, "_send", return_value=_response(404, {"error": "the audit ledger is not enabled on this node", "code": "AUDIT_DISABLED"})):
            with self.assertRaises(HTTPException) as error:
                engine_governance.audit(_Request(), admin)
        self.assertEqual(error.exception.status_code, 404)
        with patch.object(engine_bridge, "_send", return_value=_response(200, {"records": "no"})):
            with self.assertRaises(HTTPException) as error:
                engine_governance.audit(_Request(), admin)
        self.assertEqual(error.exception.status_code, 502)
        with patch.object(engine_bridge, "_send") as send:
            with self.assertRaises(HTTPException) as error:
                engine_governance.audit(_Request(), UserContext("analyst@example.com", "Analyst"))
        self.assertEqual(error.exception.status_code, 403)
        send.assert_not_called()

    def test_audit_export_streams_json_lines_as_an_attachment(self):
        admin = UserContext("admin@example.com", "Admin")
        lines = [b'{"seq":1,"kind":"statement.submitted"}\n', b'{"seq":2,"kind":"statement.finished"}\n']
        with patch.object(engine_bridge, "audit_export", return_value=iter(lines)) as export:
            response = engine_governance.audit(_Request(format="jsonl", kind="statement"), admin)
        self.assertEqual(export.call_args.args, ({"kind": "statement"}, "admin@example.com", "Admin"))
        self.assertEqual(response.media_type, "application/x-ndjson")
        self.assertEqual(response.headers["content-disposition"], 'attachment; filename="kaveon-audit.jsonl"')
        async def drain():
            return [chunk async for chunk in response.body_iterator]
        self.assertEqual(asyncio.run(drain()), lines)


if __name__ == "__main__":
    unittest.main()
