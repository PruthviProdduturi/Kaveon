import json
import os
import sqlite3
import unittest
from unittest.mock import patch
from cryptography.fernet import Fernet
from services import credentials


class SQLiteConnection:
    def __init__(self):
        self.db = sqlite3.connect(":memory:")
        self.db.execute("CREATE TABLE data_sources(id INTEGER PRIMARY KEY, connection_string TEXT)")
    def execute_query(self, sql, params):
        cursor = self.db.execute(sql, params)
        self.db.commit()
        return {"row_count": cursor.rowcount}


class CredentialTests(unittest.TestCase):
    def setUp(self):
        self.old_key, self.new_key = Fernet.generate_key().decode(), Fernet.generate_key().decode()
        self.env = patch.dict(os.environ, {"KAVEON_CREDENTIAL_KEYS": json.dumps({"old": self.old_key, "new": self.new_key}), "KAVEON_CREDENTIAL_ACTIVE_KEY": "old"})
        self.env.start()
        self.addCleanup(self.env.stop)

    def test_ciphertext_roundtrip_and_tamper_rejected(self):
        encoded = credentials.encrypt("postgres://alice:secret@localhost/data")
        self.assertNotIn("secret", encoded)
        self.assertEqual(credentials.decrypt(encoded), "postgres://alice:secret@localhost/data")
        with self.assertRaises(credentials.CredentialError):
            credentials.decrypt(encoded[:-10] + "corrupted")

    def test_missing_key_fails_closed_for_writes_and_legacy_reads(self):
        with patch.dict(os.environ, {"KAVEON_CREDENTIAL_KEYS": "{}"}):
            for operation in [lambda: credentials.encrypt("plaintext"), lambda: credentials.upgrade("plaintext")]:
                with self.assertRaises(credentials.CredentialError): operation()

    def test_plaintext_migration_and_key_rotation_are_persisted_idempotently(self):
        connection = SQLiteConnection()
        self.addCleanup(connection.db.close)
        connection.db.execute("INSERT INTO data_sources VALUES (1, 'legacy-password')")
        row = {"id": 1, "connection_string": "legacy-password"}
        self.assertEqual(credentials.source_for_use(row, connection, "?")["connection_string"], "legacy-password")
        encrypted = connection.db.execute("SELECT connection_string FROM data_sources").fetchone()[0]
        self.assertTrue(encrypted.startswith(credentials.PREFIX + "old:"))
        row["connection_string"] = encrypted
        credentials.source_for_use(row, connection, "?")
        self.assertEqual(connection.db.execute("SELECT connection_string FROM data_sources").fetchone()[0], encrypted)
        os.environ["KAVEON_CREDENTIAL_ACTIVE_KEY"] = "new"
        credentials.source_for_use(row, connection, "?")
        rotated = connection.db.execute("SELECT connection_string FROM data_sources").fetchone()[0]
        self.assertTrue(rotated.startswith(credentials.PREFIX + "new:"))
        with patch.dict(os.environ, {"KAVEON_CREDENTIAL_KEYS": json.dumps({"new": self.new_key})}):
            self.assertEqual(credentials.decrypt(rotated), "legacy-password")
            with self.assertRaises(credentials.CredentialError): credentials.decrypt(encrypted)

    def test_concurrent_credential_change_is_not_overwritten(self):
        connection = SQLiteConnection()
        self.addCleanup(connection.db.close)
        connection.db.execute("INSERT INTO data_sources VALUES (1, 'new-password')")
        with self.assertRaises(credentials.CredentialError):
            credentials.source_for_use({"id": 1, "connection_string": "stale-password"}, connection, "?")
        self.assertEqual(connection.db.execute("SELECT connection_string FROM data_sources").fetchone()[0], "new-password")

    def test_key_vault_envelope_is_default_off_and_never_falls_back(self):
        row={"id":1,"connection_string":credentials.KEY_VAULT_PREFIX+"https://unit.vault.azure.net/secrets/name/version"}
        with patch.dict(os.environ,{"KAVEON_SOURCE_SECRET_READ_ENABLED":"false"}):
            with self.assertRaisesRegex(credentials.CredentialError,"disabled"):
                credentials.source_for_use(row,None,"?")

    def test_key_vault_envelope_resolves_without_postgresql_rewrite(self):
        reference="https://unit.vault.azure.net/secrets/name/version";row={"id":1,"connection_string":credentials.KEY_VAULT_PREFIX+reference}
        resolver=unittest.mock.Mock();resolver.get.return_value="postgres://private"
        with patch.dict(os.environ,{"KAVEON_SOURCE_SECRET_READ_ENABLED":"true"}),patch("services.source_secret_store.SourceSecretStore",return_value=resolver):
            result=credentials.source_for_use(row,None,"?")
        self.assertEqual(result["connection_string"],"postgres://private");resolver.get.assert_called_once_with(reference)

    def test_key_vault_resolution_error_is_content_free(self):
        sensitive="remote-secret-body";row={"id":1,"connection_string":credentials.KEY_VAULT_PREFIX+"https://unit.vault.azure.net/secrets/name"}
        resolver=unittest.mock.Mock();resolver.get.side_effect=RuntimeError(sensitive)
        with patch.dict(os.environ,{"KAVEON_SOURCE_SECRET_READ_ENABLED":"true"}),patch("services.source_secret_store.SourceSecretStore",return_value=resolver):
            with self.assertRaises(credentials.CredentialError) as raised:credentials.source_for_use(row,None,"?")
        self.assertNotIn(sensitive,str(raised.exception))


class LegacyCiphertextTests(unittest.TestCase):
    def test_explicit_legacy_migration_and_no_runtime_fallback(self):
        import base64
        import hashlib
        from services import credentials, auth_config
        secret = "explicit-old-secret"
        legacy = Fernet(base64.urlsafe_b64encode(hashlib.sha256(secret.encode()).digest())).encrypt(b"private-key").decode()
        key = Fernet.generate_key().decode()
        with patch.dict("os.environ", {"KAVEON_CREDENTIAL_KEYS": json.dumps({"new": key}), "KAVEON_CREDENTIAL_ACTIVE_KEY": "new"}):
            with self.assertRaises(credentials.CredentialError):
                auth_config._decrypt(legacy)
            with self.assertRaises(credentials.CredentialError):
                credentials.migrate_legacy_ciphertext(legacy, "")
            with self.assertRaises(credentials.CredentialError):
                credentials.migrate_legacy_ciphertext(legacy, "wrong")
            migrated = credentials.migrate_legacy_ciphertext(legacy, secret)
            self.assertEqual(auth_config._decrypt(migrated), "private-key")
        with patch.dict("os.environ", {}, clear=True):
            with self.assertRaises(credentials.CredentialError):
                auth_config._encrypt("private-key")


    def test_auth_file_migration_dry_run_and_write(self):
        import base64
        import hashlib
        import tempfile
        from pathlib import Path
        from services import migrate_credentials, auth_config
        secret = "explicit-old-secret"
        legacy = Fernet(base64.urlsafe_b64encode(hashlib.sha256(secret.encode()).digest())).encrypt(b"private-key").decode()
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "auth.env"
            initial = "OTHER_SETTING=keep\nAUTH_GOOGLE_CLIENT_SECRET=" + legacy + "\n"
            path.write_text(initial)
            with patch.dict("os.environ", {"KAVEON_CREDENTIAL_KEYS": json.dumps({"new": Fernet.generate_key().decode()}), "KAVEON_CREDENTIAL_ACTIVE_KEY": "new", "KAVEON_LEGACY_AI_ENCRYPTION_SECRET": secret}), patch.object(auth_config, "ENV_PATH", path):
                args = ["migration", "--scope", "auth-env", "--auth-env-path", str(path)]
                with patch("sys.argv", args):
                    self.assertEqual(migrate_credentials.main(), 0)
                self.assertEqual(path.read_text(), initial)
                with patch("sys.argv", args + ["--write"]):
                    self.assertEqual(migrate_credentials.main(), 0)
                self.assertIn("OTHER_SETTING=keep", path.read_text())
                self.assertNotIn(legacy, path.read_text())
                self.assertNotIn("private-key", path.read_text())
                self.assertEqual(auth_config._decrypt(auth_config._read_key("AUTH_GOOGLE_CLIENT_SECRET")), "private-key")


    def test_auth_default_config_path_stays_in_repository(self):
        import importlib
        from pathlib import Path
        from services import auth_config
        with patch.dict("os.environ", {}, clear=True):
            importlib.reload(auth_config)
            self.assertEqual(auth_config.ENV_PATH, Path(auth_config.__file__).resolve().parents[2] / ".env")
        importlib.reload(auth_config)
