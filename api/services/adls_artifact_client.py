"""Create-only Azure Blob/ADLS Gen2 client for immutable DLM artifacts.

The client uses Azure Entra tokens and conditional block-blob creation. It never
overwrites an existing object; callers reconcile an existing object byte-for-
byte through ``read`` before treating it as idempotent.
"""

from __future__ import annotations

import os
from urllib.parse import quote
from urllib.request import Request, urlopen

from azure.identity import DefaultAzureCredential


class AzureArtifactClient:
    def __init__(self, account: str, container: str, credential=None, opener=urlopen):
        if not account or not container:
            raise ValueError("ADLS account and container are required")
        self.account = account
        self.container = container.strip("/")
        self.credential = credential or DefaultAzureCredential()
        self._opener = opener

    @classmethod
    def from_env(cls):
        return cls(os.environ.get("KAVEON_ADLS_ACCOUNT", ""),
                   os.environ.get("KAVEON_ADLS_CONTAINER", ""))

    def _url(self, path: str) -> str:
        encoded = quote(path, safe="/")
        return f"https://{self.account}.blob.core.windows.net/{self.container}/{encoded}"

    def _request(self, method: str, path: str, body: bytes | None = None, **headers):
        token = self.credential.get_token("https://storage.azure.com/.default").token
        request_headers = {
            "Authorization": f"Bearer {token}",
            "x-ms-version": "2023-11-03",
            "x-ms-date": __import__("email.utils", fromlist=["formatdate"]).formatdate(usegmt=True),
            **headers,
        }
        request = Request(self._url(path), data=body, headers=request_headers, method=method)
        try:
            return self._opener(request)
        except Exception as error:
            status = getattr(error, "code", None)
            if status is not None:
                error.status = status
            raise

    def create_if_absent(self, path: str, content: bytes) -> None:
        response = self._request("PUT", path, content,
                                 **{"Content-Type": "application/octet-stream",
                                    "Content-Length": str(len(content)),
                                    "x-ms-blob-type": "BlockBlob",
                                    "If-None-Match": "*"})
        response.close()

    def read(self, path: str, max_bytes: int) -> bytes | None:
        try:
            response = self._request("GET", path, Range=f"bytes=0-{max_bytes}")
        except Exception as error:
            if getattr(error, "status", None) == 404:
                return None
            raise
        try:
            content = response.read(max_bytes + 1)
        finally:
            response.close()
        return content


def from_env() -> AzureArtifactClient:
    """Factory for ``scripts/backfill-dlm-runs.py --client-factory``."""
    return AzureArtifactClient.from_env()
