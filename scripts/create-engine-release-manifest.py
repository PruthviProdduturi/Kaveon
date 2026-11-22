#!/usr/bin/env python3
"""Create a deterministic manifest for Engine release artifacts.

The manifest intentionally contains no credentials and no wall-clock field.
Given the same source revision and bytes it is byte-for-byte reproducible.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--channel", default="development-preview")
    parser.add_argument("--platform", action="append", nargs=2, metavar=("NAME", "PATH"), required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    if not args.source_commit or any(c.isspace() for c in args.source_commit):
        parser.error("--source-commit must be a non-empty token")
    artifacts = []
    for name, raw_path in sorted(args.platform):
        path = Path(raw_path)
        if not path.is_file():
            parser.error(f"artifact does not exist: {path}")
        artifacts.append({"name": name, "sha256": sha256(path), "size": path.stat().st_size})

    payload = {
        "schema": "kaveon.engine.release-manifest.v1",
        "channel": args.channel,
        "source_commit": args.source_commit,
        "artifacts": artifacts,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
