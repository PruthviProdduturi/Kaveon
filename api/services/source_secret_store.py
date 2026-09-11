"""Azure Key Vault boundary for source credentials; contains no migration logic."""
import hashlib
import os
import re
from urllib.parse import urlparse

import httpx
from azure.identity import DefaultAzureCredential

API_VERSION = "7.4"
SCOPE = "https://vault.azure.net/.default"
MAX_SECRET_BYTES = 64 * 1024
MAX_RESPONSE_BYTES = 128 * 1024
TIMEOUT_SECONDS = 10.0
_NAME = re.compile(r"[A-Za-z0-9-]{1,127}")


class SourceSecretError(RuntimeError):
    pass


def vault_url(value: str | None = None) -> str:
    value = (value if value is not None else os.getenv("KAVEON_KEY_VAULT_URL", "")).rstrip("/")
    parsed = urlparse(value)
    if (parsed.scheme != "https" or not parsed.hostname or parsed.port is not None
            or parsed.username or parsed.password or parsed.path or parsed.query or parsed.fragment
            or not parsed.hostname.endswith(".vault.azure.net")
            or parsed.hostname.count(".") != 3):
        raise SourceSecretError("KAVEON_KEY_VAULT_URL is missing or invalid")
    return f"https://{parsed.hostname}"


def secret_name(source_kind: str, source_id: str) -> str:
    if source_kind not in {"catalog", "data"} or not source_id or len(source_id) > 512:
        raise SourceSecretError("source secret identity is invalid")
    digest = hashlib.sha256(f"{source_kind}\0{source_id}".encode("utf-8")).hexdigest()
    return f"kaveon-source-{source_kind}-{digest}"


def validate_reference(reference: str, configured_vault: str | None = None) -> str:
    base = vault_url(configured_vault)
    parsed = urlparse(reference)
    parts = parsed.path.split("/")
    if (parsed.scheme != "https" or parsed.username or parsed.password or parsed.port is not None
            or f"{parsed.scheme}://{parsed.hostname}" != base or parsed.query or parsed.fragment
            or len(parts) not in {3, 4} or parts[1] != "secrets"
            or not _NAME.fullmatch(parts[2]) or (len(parts) == 4 and not _NAME.fullmatch(parts[3]))):
        raise SourceSecretError("source secret reference is invalid")
    return reference


class SourceSecretStore:
    def __init__(self, *, credential=None, client=None, configured_vault=None):
        self.vault = vault_url(configured_vault)
        self.credential = credential or DefaultAzureCredential()
        self.client = client or httpx.Client(timeout=TIMEOUT_SECONDS)

    def _request(self, method: str, url: str, *, json=None) -> dict:
        try:
            token = self.credential.get_token(SCOPE).token
            response = self.client.request(method, url, params={"api-version": API_VERSION},
                                           headers={"Authorization": f"Bearer {token}"}, json=json)
            if len(response.content) > MAX_RESPONSE_BYTES: raise SourceSecretError("Key Vault response exceeded its bound")
            if response.status_code not in ({200} if method != "DELETE" else {200, 202}):
                raise SourceSecretError("Key Vault source secret operation failed")
            result = response.json()
            if not isinstance(result, dict): raise ValueError
            return result
        except SourceSecretError: raise
        except Exception:
            raise SourceSecretError("Key Vault source secret operation failed") from None

    def set(self, source_kind: str, source_id: str, value: str) -> str:
        if not isinstance(value, str) or not value or len(value.encode("utf-8")) > MAX_SECRET_BYTES:
            raise SourceSecretError("source secret value is missing or exceeds its bound")
        name = secret_name(source_kind, source_id)
        result = self._request("PUT", f"{self.vault}/secrets/{name}", json={"value": value})
        reference = result.get("id")
        if not isinstance(reference, str): raise SourceSecretError("Key Vault returned an invalid source secret reference")
        return validate_reference(reference, self.vault)

    def get(self, reference: str) -> str:
        reference = validate_reference(reference, self.vault)
        result = self._request("GET", reference)
        value = result.get("value")
        if not isinstance(value, str) or not value or len(value.encode("utf-8")) > MAX_SECRET_BYTES:
            raise SourceSecretError("Key Vault returned an invalid source secret")
        return value

    def delete(self, reference: str) -> None:
        reference = validate_reference(reference, self.vault)
        self._request("DELETE", reference)
