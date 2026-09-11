"""Verify captured immutable table-publication rehearsal evidence.

This command is intentionally offline. It never contacts ADLS, AKS, or an
Engine endpoint. A report may include base64 payloads for small fixtures; when
payloads are omitted, the report must carry independently captured content
digests and exact manifest references.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import sys
from pathlib import Path

MAX_REPORT_BYTES = 16 * 1024 * 1024
HEX64 = set("0123456789abcdef")


class VerificationError(ValueError):
    pass


def _digest(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def _sha(value: object, label: str) -> str:
    if not isinstance(value, str) or len(value) != 64 or any(c not in HEX64 for c in value):
        raise VerificationError(f"{label} must be a lowercase SHA-256 digest")
    return value


def _require(mapping: dict, key: str, label: str):
    if key not in mapping:
        raise VerificationError(f"{label} is missing {key}")
    return mapping[key]


def verify(report: dict) -> dict:
    if report.get("kind") != "kaveon.immutable_table_publication_rehearsal":
        raise VerificationError("report kind is not the immutable table rehearsal format")
    if report.get("mutations_enabled") is not False:
        raise VerificationError("qualification requires mutations_enabled=false")

    manifest = _require(report, "manifest", "report")
    if not isinstance(manifest, dict):
        raise VerificationError("manifest must be an object")
    manifest_path = _require(manifest, "path", "manifest")
    manifest_sha = _sha(_require(manifest, "sha256", "manifest"), "manifest.sha256")
    if not isinstance(manifest_path, str) or not manifest_path.endswith(".json"):
        raise VerificationError("manifest.path must end in .json")

    objects = _require(report, "objects", "report")
    if not isinstance(objects, list) or not objects:
        raise VerificationError("objects must contain at least one object")
    by_path = {}
    for index, item in enumerate(objects):
        if not isinstance(item, dict):
            raise VerificationError(f"objects[{index}] must be an object")
        path = _require(item, "path", f"objects[{index}]")
        if not isinstance(path, str) or not path.endswith(".parquet"):
            raise VerificationError(f"objects[{index}].path must end in .parquet")
        if path in by_path:
            raise VerificationError(f"duplicate immutable object path: {path}")
        captured = _sha(_require(item, "sha256", f"objects[{index}]"), f"objects[{index}].sha256")
        content_b64 = item.get("content_base64")
        if content_b64 is not None:
            try:
                content = base64.b64decode(content_b64, validate=True)
            except (ValueError, TypeError) as error:
                raise VerificationError(f"objects[{index}].content_base64 is invalid") from error
            if _digest(content) != captured:
                raise VerificationError(f"content digest mismatch for {path}")
            if len(content) < 8 or content[:4] != b"PAR1" or content[-4:] != b"PAR1":
                raise VerificationError(f"captured object is not Parquet: {path}")
            if item.get("size_bytes") != len(content):
                raise VerificationError(f"size mismatch for {path}")
        elif not isinstance(item.get("size_bytes"), int) or item["size_bytes"] < 8:
            raise VerificationError(f"{path} needs a captured size_bytes or content_base64")
        by_path[path] = captured

    refs = _require(manifest, "parquet_files", "manifest")
    if not isinstance(refs, list) or not refs:
        raise VerificationError("manifest.parquet_files must be non-empty")
    for index, ref in enumerate(refs):
        if not isinstance(ref, dict):
            raise VerificationError(f"manifest.parquet_files[{index}] must be an object")
        path = _require(ref, "path", f"manifest.parquet_files[{index}]")
        digest = _sha(_require(ref, "sha256", f"manifest.parquet_files[{index}]"), "manifest parquet sha256")
        if by_path.get(path) != digest:
            raise VerificationError(f"manifest reference does not match object: {path}")

    operations = _require(report, "operations", "report")
    if not isinstance(operations, list):
        raise VerificationError("operations must be a list")
    required = {"committed", "replayed", "divergent_conflict", "stale_cas_conflict"}
    seen = {item.get("name") for item in operations if isinstance(item, dict)}
    missing = required - seen
    if missing:
        raise VerificationError(f"missing operation evidence: {', '.join(sorted(missing))}")
    for item in operations:
        if not isinstance(item, dict):
            raise VerificationError("operation evidence must be an object")
        name = item.get("name")
        outcome = item.get("outcome")
        expected = {
            "committed": "committed",
            "replayed": "replayed",
            "divergent_conflict": "conflict",
            "stale_cas_conflict": "conflict",
        }.get(name)
        if expected and outcome != expected:
            raise VerificationError(f"{name} outcome must be {expected}")
        if name in {"replayed", "divergent_conflict", "stale_cas_conflict"} and item.get("head_unchanged") is not True:
            raise VerificationError(f"{name} must prove head_unchanged=true")

    restart = _require(report, "restart", "report")
    if not isinstance(restart, dict) or restart.get("reopened") is not True:
        raise VerificationError("restart.reopened=true is required")
    if _sha(_require(restart, "head_sha256", "restart"), "restart.head_sha256") != _sha(
        _require(report, "committed_head_sha256", "report"), "committed_head_sha256"
    ):
        raise VerificationError("restart head digest differs from committed head")
    if restart.get("journal_outcome") != "committed":
        raise VerificationError("restart.journal_outcome must be committed")

    return {
        "status": "passed",
        "mutations_enabled": False,
        "manifest_sha256": manifest_sha,
        "object_count": len(by_path),
        "operation_count": len(operations),
        "restart_recovered": True,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report", required=True, type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args(argv)
    try:
        if args.report.stat().st_size > MAX_REPORT_BYTES:
            raise VerificationError("report exceeds the 16 MiB bound")
        report = json.loads(args.report.read_text(encoding="utf-8"))
        result = verify(report)
    except (OSError, json.JSONDecodeError, VerificationError) as error:
        result = {"status": "failed", "error": str(error)}
        print(json.dumps(result, sort_keys=True))
        return 2
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
