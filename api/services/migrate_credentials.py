"""Explicit credential migration: python -m services.migrate_credentials --scope auth-env [--write].

Dry-run validates every decryption without writing. Supply
KAVEON_LEGACY_AI_ENCRYPTION_SECRET explicitly.
"""
import argparse
import os
from services.credentials import CredentialError, migrate_legacy_ciphertext


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scope", required=True, choices=["auth-env"])
    parser.add_argument("--write", action="store_true")
    parser.add_argument("--auth-env-path", help="Explicit existing auth configuration file for auth-env migration")
    args = parser.parse_args()
    secret = os.environ.get("KAVEON_LEGACY_AI_ENCRYPTION_SECRET", "")
    try:
        from services import auth_config
        from pathlib import Path
        if not args.auth_env_path or not Path(args.auth_env_path).is_file():
            raise CredentialError("Explicit existing auth environment path is required")
        auth_config.ENV_PATH = Path(args.auth_env_path).resolve()
        original = auth_config._read_key("AUTH_GOOGLE_CLIENT_SECRET")
        changes = [(original, migrate_legacy_ciphertext(original, secret))] if original else []
        if args.write and changes:
            if auth_config._read_key("AUTH_GOOGLE_CLIENT_SECRET") != original:
                raise CredentialError("Auth config changed during migration")
            auth_config._upsert_env({"AUTH_GOOGLE_CLIENT_SECRET": changes[0][1]})
        print(("Migrated " if args.write else "Validated ") + str(len(changes)) + " credential(s).")
        return 0
    except Exception:
        # Exceptions can carry ciphertext.
        print("Migration failed; verify keyring, explicit legacy secret, and file access.")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
