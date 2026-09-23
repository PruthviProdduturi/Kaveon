"""Verify the local KaveonDB product catalog's head, journal, and live records."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "catalog",
        nargs="?",
        type=Path,
        default=Path("data/adls-mirror/product-transactions/kaveon/product-catalog"),
        help="local directory containing head.json (default: %(default)s)",
    )
    root = parser.parse_args().catalog.resolve()
    head_path = root / "head.json"
    try:
        head = json.loads(head_path.read_text(encoding="utf-8"))
        snapshot_ref = head["reference"]
        snapshot_path = root / "snapshots" / f"{snapshot_ref['snapshot_id']}.json"
        if not snapshot_path.is_file() or sha256(snapshot_path) != head["snapshot_sha256"]:
            raise ValueError("current snapshot is missing or its SHA-256 does not match head.json")
        snapshot = json.loads(snapshot_path.read_text(encoding="utf-8"))
    except (OSError, KeyError, TypeError, ValueError, json.JSONDecodeError) as exc:
        print(f"FAIL: cannot verify current KaveonDB snapshot: {exc}")
        return 1

    references: list[tuple[str, str, str]] = []
    for shard in head.get("operation_index", {}).values():
        references.append(("journal", shard["path"], shard["sha256"]))
    for key, record in snapshot.get("product_records", {}).items():
        document = record.get("document", {})
        if document.get("path") and document.get("sha256"):
            references.append((f"product:{record.get('kind', key.split('/', 1)[0])}",
                               document["path"], document["sha256"]))

    missing: list[tuple[str, str]] = []
    mismatches: list[tuple[str, str]] = []
    for kind, relative, expected in references:
        path = (root / relative).resolve()
        if not path.is_relative_to(root):
            mismatches.append((kind, "unsafe referenced path"))
        elif not path.is_file():
            missing.append((kind, relative))
        elif sha256(path) != expected:
            mismatches.append((kind, relative))

    print(
        f"generation={snapshot.get('generation')} "
        f"product_records={len(snapshot.get('product_records', {}))} "
        f"journal_shards={len(head.get('operation_index', {}))} "
        f"checked_objects={len(references)} missing={len(missing)} sha256_mismatch={len(mismatches)}"
    )
    for kind, path in (missing + mismatches)[:20]:
        print(f"MISSING {kind}: {path}" if (kind, path) in missing else f"HASH MISMATCH {kind}: {path}")
    if missing or mismatches:
        print("FAIL: local product mirror is incomplete; resume the ADLS mirror before starting Studio tests.")
        return 1
    print("PASS: all current catalog, journal and product-document references are present and verified.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
