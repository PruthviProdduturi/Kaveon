"""Explicit credential migration: python -m services.migrate_credentials --scope ai-db|auth-env [--write].

Uses configured metadata storage only when invoked by an operator. Dry-run validates
all decryptions without writing. Supply KAVEON_LEGACY_AI_ENCRYPTION_SECRET explicitly.
"""
import argparse
import os
from services.credentials import CredentialError, migrate_legacy_ciphertext


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scope", required=True, choices=["ai-db", "auth-env"])
    parser.add_argument("--write", action="store_true")
    parser.add_argument("--auth-env-path", help="Explicit existing auth configuration file for auth-env migration")
    args = parser.parse_args()
    secret = os.environ.get("KAVEON_LEGACY_AI_ENCRYPTION_SECRET", "")
    try:
        if args.scope == "auth-env":
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
        else:
            import database.metadata as db
            changes = []
            for table, keys in [("ai_providers", ["id"]), ("user_ai_keys", ["user_email", "provider"])]:
                rows = db.query("SELECT " + ", ".join(keys) + ", api_key_enc FROM " + table)["rows"]
                for row in rows:
                    replacement = migrate_legacy_ciphertext(row["api_key_enc"], secret)
                    conditions = " AND ".join(key + "=@param" + str(i+1) for i, key in enumerate(keys))
                    sql = "UPDATE " + table + " SET api_key_enc=@param0 WHERE " + conditions + " AND api_key_enc=@param" + str(len(keys)+1)
                    changes.append((sql, [replacement] + [row[key] for key in keys] + [row["api_key_enc"]]))
            if args.write:
                for sql, params in changes:
                    if db.execute(sql, params) != 1:
                        raise CredentialError("Credential changed during migration; rerun after resolving concurrent edits")
        print(("Migrated " if args.write else "Validated ") + str(len(changes)) + " credential(s).")
        return 0
    except Exception:
        # Database exceptions can contain bound ciphertext or connection credentials.
        print("Migration failed; verify keyring, explicit legacy secret, storage access, and concurrent edits.")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
