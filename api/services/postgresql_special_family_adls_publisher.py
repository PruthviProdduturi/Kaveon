"""ADLS adapter for lossless special-family publication."""

from __future__ import annotations

import hashlib
import json



class Publisher:
    def __init__(self, client, prefix):
        if (not isinstance(prefix, str) or not prefix.strip("/")
                or any(part in {"", ".", ".."} for part in prefix.strip("/").split("/"))):
            raise RuntimeError("special-family ADLS prefix is invalid")
        self.client, self.prefix = client, prefix.strip("/")

    def _path(self, relative): return f"{self.prefix}/{relative}"

    def publish_immutable(self, path, body, sha256):
        full = self._path(path); status = "created"
        try:
            self.client.create_if_absent_with_etag(full, body)
        except Exception as error:
            if getattr(error, "status", getattr(error, "code", None)) not in (409, 412):
                raise RuntimeError("special-family ADLS immutable write failed") from error
            status = "verified-replay"
        observed = self.client.read(full, len(body))
        if observed != body or hashlib.sha256(observed or b"").hexdigest() != sha256:
            raise RuntimeError("special-family ADLS immutable readback mismatch")
        document = json.loads(observed)
        table = document["table"]
        return {"path": path, "sha256": sha256, "bytes": len(body), "status": status,
                "row_count": table["row_count"], "key_set_sha256": table["key_sha256"],
                "content_sha256": table["content_sha256"]}

    def publish_manifest(self, body, *, expected_head, max_attempts):
        digest = hashlib.sha256(body).hexdigest()
        manifest_path = self._path(f"manifests/{digest}.json")
        try:
            self.client.create_if_absent_with_etag(manifest_path, body)
        except Exception as error:
            if getattr(error, "status", getattr(error, "code", None)) not in (409, 412):
                raise RuntimeError("special-family ADLS manifest write failed") from error
        if self.client.read(manifest_path, len(body)) != body:
            raise RuntimeError("special-family ADLS manifest readback mismatch")
        head_path = self._path("head.json")
        head_body = json.dumps({"manifest_path": f"manifests/{digest}.json", "sha256": digest},
                               sort_keys=True, separators=(",", ":")).encode()
        current = self.client.read_with_etag(head_path, len(head_body))
        if current is not None and current[0] == head_body:
            return {"sha256": digest, "status": "verified-replay", "cas_attempts": 0,
                    "published_last": True, "head_etag": current[1]}
        prior = None if expected_head == "absent" else expected_head
        if (expected_head == "absent" and current is not None) or (
                expected_head != "absent" and (current is None or current[1] != expected_head)):
            raise RuntimeError("special-family ADLS head changed concurrently")
        attempts = 0
        while attempts < max_attempts:
            attempts += 1
            try:
                etag = self.client.put_if_match(head_path, head_body, prior)
                confirmed = self.client.read_with_etag(head_path, len(head_body))
                if confirmed is None or confirmed[0] != head_body or confirmed[1] != etag:
                    raise RuntimeError("special-family ADLS head readback mismatch")
                return {"sha256": digest, "status": "committed", "cas_attempts": attempts,
                        "published_last": True, "head_etag": etag}
            except Exception as error:
                # An ambiguous response may have committed. Exact readback is the
                # only accepted replay; a different head is a hard CAS conflict.
                confirmed = self.client.read_with_etag(head_path, len(head_body))
                if confirmed is not None and confirmed[0] == head_body:
                    return {"sha256": digest, "status": "verified-replay",
                            "cas_attempts": attempts, "published_last": True,
                            "head_etag": confirmed[1]}
                status = getattr(error, "status", getattr(error, "code", None))
                if status in (409, 412) or confirmed is not None:
                    raise RuntimeError("special-family ADLS head changed concurrently") from error
                if attempts == max_attempts:
                    raise RuntimeError("special-family ADLS head CAS retry bound exceeded") from error
        raise RuntimeError("special-family ADLS head CAS retry bound exceeded")
