"""Versioned encryption with explicit key IDs and no public/default key fallback."""
import json
import os
import re
from cryptography.fernet import Fernet, InvalidToken

PREFIX = "kaveon:fernet:v1:"


class CredentialError(RuntimeError):
    pass


def _keyring():
    try:
        encoded = json.loads(os.environ["KAVEON_CREDENTIAL_KEYS"])
        active = os.environ["KAVEON_CREDENTIAL_ACTIVE_KEY"]
        if not isinstance(encoded, dict) or active not in encoded:
            raise ValueError
        keys = {}
        for key_id, key in encoded.items():
            if not re.fullmatch(r"[A-Za-z0-9_-]{1,64}", key_id):
                raise ValueError
            keys[key_id] = Fernet(key.encode("ascii"))
        return active, keys
    except (KeyError, ValueError, TypeError, AttributeError, UnicodeError):
        raise CredentialError("Credential encryption keyring is missing or invalid") from None


def encrypt(value: str) -> str:
    if not isinstance(value, str) or not value:
        raise CredentialError("Credential must be a nonempty string")
    active, keys = _keyring()
    return PREFIX + active + ":" + keys[active].encrypt(value.encode()).decode("ascii")


def decrypt(value: str) -> str:
    if not isinstance(value, str) or not value.startswith(PREFIX):
        raise CredentialError("Credential is not a supported encrypted envelope")
    _, keys = _keyring()
    try:
        key_id, token = value[len(PREFIX):].split(":", 1)
        return keys[key_id].decrypt(token.encode("ascii")).decode()
    except (KeyError, ValueError, InvalidToken, UnicodeError):
        raise CredentialError("Credential cannot be decrypted with the configured keyring") from None


def upgrade(value: str) -> tuple[str, str | None]:
    """Return plaintext and replacement envelope; persist replacement before use.

    The sole legacy plaintext read path exists to perform a CAS migration. It
    requires valid encryption keys even for legacy input, preventing silent use
    of plaintext when the key manager is unavailable.
    """
    active, _ = _keyring()
    if value.startswith("kaveon:") and not value.startswith(PREFIX):
        raise CredentialError("Unsupported credential envelope version")
    plaintext = decrypt(value) if value.startswith(PREFIX) else value
    if value.startswith(PREFIX + active + ":"):
        return plaintext, None
    return plaintext, encrypt(plaintext)


def source_for_use(row, connection, placeholder):
    """Migrate/rotate atomically using the existing metadata connection."""
    if not row:
        return None
    value = row.get("connection_string") or ""
    plaintext, replacement = upgrade(value)
    if replacement is not None:
        try:
            result = connection.execute_query(
                f"UPDATE data_sources SET connection_string = {placeholder} "
                f"WHERE id = {placeholder} AND connection_string = {placeholder}",
                [replacement, row["id"], value],
            )
        except Exception:
            raise CredentialError("Credential migration could not be persisted") from None
        if result.get("row_count") != 1:
            raise CredentialError("Data source changed during credential migration; retry the request")
    return {**row, "connection_string": plaintext}


def migrate_legacy_ciphertext(value: str, legacy_secret: str) -> str:
    """Offline migration with an explicitly supplied original secret.

    No default or tenant-derived secret is guessed. Runtime decrypt rejects legacy.
    """
    import base64
    import hashlib
    _keyring()
    if value.startswith(PREFIX):
        plaintext = decrypt(value)
    else:
        if not legacy_secret:
            raise CredentialError("Explicit legacy encryption secret is required")
        try:
            key = base64.urlsafe_b64encode(hashlib.sha256(legacy_secret.encode()).digest())
            plaintext = Fernet(key).decrypt(value.encode()).decode()
        except (InvalidToken, ValueError, UnicodeError):
            raise CredentialError("Legacy credential cannot be decrypted with the supplied secret") from None
    return encrypt(plaintext)
