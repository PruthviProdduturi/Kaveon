import unittest
from unittest.mock import patch
from starlette.requests import Request

from middleware import auth
from services import engine_bridge

TENANT = "11111111-1111-1111-1111-111111111111"
OID = "22222222-2222-2222-2222-222222222222"
PRINCIPAL = f"entra:{TENANT}:{OID}"


def request(**headers):
    return Request({"type": "http", "headers": [(k.replace("_", "-").encode(), v.encode()) for k, v in headers.items()]})


class EntraAccessTests(unittest.TestCase):
    def setUp(self):
        for target, key, val in [(auth, "_TENANT_ID", TENANT), (auth.settings, "KAVEON_ENTRA_APP_ROLES", True),
                                 (auth.settings, "KAVEON_PROXY_SECRET", "test-secret")]:
            mock = patch.object(target, key, val)
            mock.start()
            self.addCleanup(mock.stop)

    def test_studio_requires_trusted_session_and_stable_tenant_identity(self):
        headers = dict(x_user_email="Owner@org.example", x_user_role="Editor", x_user_entra_principal=PRINCIPAL)
        self.assertIsNone(auth.trusted_directory_request(request(**headers)))
        req = request(**headers, x_proxy_secret="test-secret")
        req.state.directory_caller = auth.trusted_directory_request(req)
        self.assertEqual(req.state.directory_caller, ("owner@org.example", "Editor", PRINCIPAL, ()))
        self.assertEqual(auth.get_user_context(req).role, "Editor")
        headers["x_user_entra_principal"] = f"entra:{OID}:{TENANT}"
        self.assertIsNone(auth.trusted_directory_request(request(**headers, x_proxy_secret="test-secret")))

    def test_fresh_token_with_revoked_roles_denies_email_and_role_routes(self):
        claims = dict(tid=TENANT, oid=OID, email="owner@org.example", roles=["Viewer", "Analyst", "Kaveon.Query.OpenSource", "Kaveon.Query.KaveonDB"], scp="access_as_user")
        with patch.object(auth, "_azure_claims", return_value=claims):
            req = request(authorization="Bearer fixture")
            req.state.directory_caller = auth.trusted_directory_request(req)
            self.assertEqual(auth.get_user_context(req).role, "Analyst")
            self.assertEqual(req.state.directory_caller[3], ("OpenSource",))
            claims["roles"] = []
            req.state.directory_caller = auth.trusted_directory_request(req)
            self.assertIsNone(auth.get_current_user(req))
            self.assertIsNone(auth.get_user_context(req))
            claims.update(roles=["Admin"], scp="other")
            self.assertIsNone(auth.trusted_directory_request(req))

    def test_engine_receives_stable_subject_only_for_the_authenticated_actor(self):
        token = auth.entra_caller.set(("owner@org.example", PRINCIPAL, ("OpenSource",)))
        try:
            self.assertEqual(engine_bridge._caller_headers("owner@org.example", "analyst")["x-kaveon-principal"], PRINCIPAL)
            self.assertEqual(engine_bridge._caller_headers("background", "admin")["x-kaveon-principal"], "background")
        finally:
            auth.entra_caller.reset(token)
