import unittest
from unittest.mock import patch
from fastapi import FastAPI, Depends
from fastapi.testclient import TestClient
from middleware import auth
from middleware.permissions import require_min_role


class AuthenticationTests(unittest.TestCase):
    def setUp(self):
        app = FastAPI()
        @app.get("/admin")
        def admin(ctx=Depends(require_min_role("Admin"))):
            return {"role": ctx.role}
        @app.get("/context")
        def context(ctx=Depends(auth.require_user_context)):
            return {"role": ctx.role}
        self.client = TestClient(app)

    def test_role_database_failure_cannot_grant_admin_without_aad(self):
        with patch.object(auth, "_proxy_identity", return_value=None), patch.object(auth, "_decode_token", return_value=("alice@example.test", [])), patch.object(auth, "_get_active_provider", return_value="google"), patch.object(auth, "_AAD_CONFIGURED", False), patch("services.users.resolve_role", side_effect=RuntimeError("unavailable")):
            headers = {"Authorization": "Bearer test"}
            self.assertEqual(self.client.get("/admin", headers=headers).status_code, 403)
            self.assertEqual(self.client.get("/context", headers=headers).status_code, 403)

    def test_unstamped_identity_headers_do_not_authenticate(self):
        with patch.object(auth.settings, "KAVEON_DEV_USER_EMAIL", ""), patch.object(auth.settings, "KAVEON_PROXY_SECRET", "a-real-proxy-secret"):
            self.assertEqual(self.client.get("/admin", headers={"x-user-email": "admin@example.test", "x-user-role": "Admin"}).status_code, 401)

    def test_production_ignores_dev_identity_bypass(self):
        with patch.object(auth.settings, "KAVEON_DEV_USER_EMAIL", "dev@example.test"), patch.object(auth.settings, "NODE_ENV", "production"), patch.object(auth.settings, "KAVEON_PROXY_SECRET", ""):
            self.assertEqual(self.client.get("/admin").status_code, 401)

    def test_unrecognized_proxy_role_is_denied(self):
        with patch.object(auth.settings, "KAVEON_DEV_USER_EMAIL", ""), patch.object(auth.settings, "KAVEON_PROXY_SECRET", "a-real-proxy-secret"):
            headers = {"x-proxy-secret": "a-real-proxy-secret", "x-user-email": "alice@example.test", "x-user-role": "Owner"}
            self.assertEqual(self.client.get("/context", headers=headers).status_code, 403)


class SignedTokenTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        from cryptography.hazmat.primitives.asymmetric import rsa
        cls.key = rsa.generate_private_key(public_exponent=65537, key_size=2048)

    def test_azure_signature_issuer_tenant_and_required_claims(self):
        import time
        import jwt
        from types import SimpleNamespace
        tenant = "11111111-1111-1111-1111-111111111111"
        claims = {"sub": "subject", "tid": tenant, "iss": f"https://login.microsoftonline.com/{tenant}/v2.0", "aud": "client", "exp": int(time.time()) + 60, "email": "alice@example.test"}
        client = SimpleNamespace(get_signing_key_from_jwt=lambda token: SimpleNamespace(key=self.key.public_key()))
        with patch.object(auth, "_aad_jwks_client", client), patch.object(auth.settings, "AZURE_TENANT_ID", tenant), patch.object(auth.settings, "AZURE_CLIENT_ID", "client"):
            self.assertEqual(auth._decode_azure_ad(jwt.encode(claims, self.key, algorithm="RS256")), ("alice@example.test", []))
            for changed in [{**claims, "iss": "https://evil.test"}, {**claims, "tid": "other"}, {**claims, "aud": "other"}, {k: v for k, v in claims.items() if k != "exp"}]:
                self.assertIsNone(auth._decode_azure_ad(jwt.encode(changed, self.key, algorithm="RS256")))

    def test_google_requires_issuer_and_verified_email(self):
        import time
        import jwt
        from types import SimpleNamespace
        claims = {"sub": "subject", "iss": "https://accounts.google.com", "aud": "client", "exp": int(time.time()) + 60, "email": "alice@example.test", "email_verified": True}
        client = SimpleNamespace(get_signing_key_from_jwt=lambda token: SimpleNamespace(key=self.key.public_key()))
        with patch.object(auth, "_get_google_jwks_client", return_value=client), patch("services.auth_config.get_config", return_value={"google_client_id": "client"}):
            self.assertEqual(auth._decode_google(jwt.encode(claims, self.key, algorithm="RS256")), ("alice@example.test", []))
            for changed in [{**claims, "iss": "https://evil.test"}, {**claims, "email_verified": False}, {**claims, "email_verified": "true"}, {k: v for k, v in claims.items() if k != "exp"}]:
                self.assertIsNone(auth._decode_google(jwt.encode(changed, self.key, algorithm="RS256")))
