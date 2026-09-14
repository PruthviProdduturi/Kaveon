"""Create-only Azure Blob/ADLS Gen2 client for immutable DLM artifacts.

The client uses Azure Entra tokens and conditional block-blob creation. It never
overwrites an existing object; callers reconcile an existing object byte-for-
byte through ``read`` before treating it as idempotent.
"""

from __future__ import annotations

import os
import re
from urllib.parse import quote
from urllib.parse import urlencode
from urllib.request import Request, urlopen

from azure.identity import DefaultAzureCredential


class AzureArtifactClient:
    def __init__(self, account: str, container: str, credential=None, opener=urlopen):
        if re.fullmatch(r"[a-z0-9]{3,24}", str(account or "")) is None:
            raise ValueError("ADLS account name is invalid")
        normalized_container = str(container or "").strip("/")
        if (not 3 <= len(normalized_container) <= 63
                or re.fullmatch(r"[a-z0-9](?:[a-z0-9-]*[a-z0-9])", normalized_container) is None
                or "--" in normalized_container):
            raise ValueError("ADLS container name is invalid")
        self.account = str(account)
        self.container = normalized_container
        self.credential = credential or DefaultAzureCredential()
        self._opener = opener

    @classmethod
    def from_env(cls):
        return cls(os.environ.get("KAVEON_ADLS_ACCOUNT", ""),
                   os.environ.get("KAVEON_ADLS_CONTAINER", ""))

    def _url(self, path: str) -> str:
        self._validate_path(path)
        encoded = quote(path, safe="/")
        return f"https://{self.account}.blob.core.windows.net/{self.container}/{encoded}"

    @staticmethod
    def _validate_path(path: str) -> None:
        if (not isinstance(path, str) or not path or len(path.encode("utf-8")) > 2048
                or path.startswith("/") or any(ord(character) < 32 for character in path)
                or any(part in {"", ".", ".."} for part in path.split("/"))):
            raise RuntimeError("ADLS object path is invalid")

    def _request(self, method: str, path: str, body: bytes | None = None, **headers):
        # Validate the destination before obtaining a bearer token. A malformed
        # path must never cause credential acquisition or an outbound request.
        self._validate_path(path)
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
            if status is not None and getattr(error, "status", None) is None:
                error.status = status
            raise

    def create_if_absent(self, path: str, content: bytes) -> None:
        response = self._request("PUT", path, content,
                                 **{"Content-Type": "application/octet-stream",
                                    "Content-Length": str(len(content)),
                                    "x-ms-blob-type": "BlockBlob",
                                    "If-None-Match": "*"})
        response.close()

    def create_if_absent_with_etag(self, path: str, content: bytes) -> str:
        response = self._request("PUT", path, content,
                                 **{"Content-Type": "application/octet-stream",
                                    "Content-Length": str(len(content)),
                                    "x-ms-blob-type": "BlockBlob",
                                    "If-None-Match": "*"})
        etag = response.headers.get("ETag")
        response.close()
        if not etag:
            raise RuntimeError("ADLS create-only write returned no ETag")
        return etag

    def read(self, path: str, max_bytes: int) -> bytes | None:
        try:
            headers = {} if max_bytes == 0 else {"Range": f"bytes=0-{max_bytes}"}
            response = self._request("GET", path, **headers)
        except Exception as error:
            if getattr(error, "status", None) == 404:
                return None
            raise
        try:
            content = response.read(max_bytes + 1)
        finally:
            response.close()
        return content

    def delete_if_match(self, path: str, etag: str) -> None:
        if (not isinstance(etag, str) or not etag or len(etag) > 256
                or any(ord(character) < 32 for character in etag)):
            raise RuntimeError("ADLS cleanup requires an ETag")
        response = self._request("DELETE", path, **{"If-Match": etag})
        response.close()

    def list(self, prefix: str, max_objects: int = 10000) -> list[dict]:
        import xml.etree.ElementTree as ET
        self._validate_path(prefix)
        if not 1 <= max_objects <= 10000:
            raise RuntimeError("ADLS list prefix or bound is invalid")
        values, marker = [], None
        while True:
            query={"restype":"container","comp":"list","include":"metadata",
                   "prefix":prefix.rstrip("/")+"/","maxresults":"5000"}
            if marker: query["marker"]=marker
            token=self.credential.get_token("https://storage.azure.com/.default").token
            request=Request(f"https://{self.account}.blob.core.windows.net/{self.container}?{urlencode(query)}",
                headers={"Authorization":f"Bearer {token}","x-ms-version":"2023-11-03",
                         "x-ms-date":__import__("email.utils",fromlist=["formatdate"]).formatdate(usegmt=True)},method="GET")
            response=self._opener(request)
            try:
                payload=response.read(4*1024*1024+1)
                if len(payload)>4*1024*1024:raise RuntimeError("ADLS list response is oversized")
                root=ET.fromstring(payload)
            except Exception as error: raise RuntimeError("ADLS list returned invalid XML") from error
            finally: response.close()
            for blob in root.findall("./Blobs/Blob"):
                metadata = blob.find("Metadata")
                is_directory = metadata is not None and any(
                    child.tag.lower() == "hdi_isfolder" and (child.text or "").lower() == "true"
                    for child in metadata
                )
                if is_directory:
                    continue
                name=blob.findtext("Name");etag=blob.findtext("./Properties/Etag");size=blob.findtext("./Properties/Content-Length")
                try: size=int(size)
                except (TypeError,ValueError): raise RuntimeError("ADLS list returned invalid object metadata") from None
                if not name or not etag or size < 0: raise RuntimeError("ADLS list returned invalid object metadata")
                values.append({"path":name,"etag":etag,"size":size})
                if len(values)>max_objects: raise RuntimeError("ADLS list exceeds its object bound")
            marker=root.findtext("NextMarker") or None
            if not marker:return values


def from_env() -> AzureArtifactClient:
    """Factory for ``scripts/backfill-dlm-runs.py --client-factory``."""
    return AzureArtifactClient.from_env()
