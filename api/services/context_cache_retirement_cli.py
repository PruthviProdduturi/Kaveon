"""API-image entrypoint for a verified context-cache retirement report."""

import argparse
import json
import os
import tempfile
from datetime import datetime, timezone
from pathlib import Path

from services import context_cache_retirement


def _atomic(path: Path, value: dict) -> None:
    path = path.resolve(); path.parent.mkdir(parents=True, exist_ok=True); temporary = None
    try:
        with tempfile.NamedTemporaryFile("wb", dir=path.parent, prefix=path.name + ".",
                                         delete=False) as handle:
            temporary = Path(handle.name); os.chmod(temporary, 0o600)
            handle.write(json.dumps(value, sort_keys=True, separators=(",", ":")).encode())
            handle.flush(); os.fsync(handle.fileno())
        os.replace(temporary, path)
    finally:
        if temporary and temporary.exists(): temporary.unlink()


def main() -> int:
    parser = argparse.ArgumentParser(description="Verify live rebuild/delete observations and emit context_cache.json")
    parser.add_argument("--evidence", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--max-age-hours", type=int, default=1)
    args = parser.parse_args()
    if not args.evidence.is_file() or args.evidence.stat().st_size > context_cache_retirement.MAX_EVIDENCE_BYTES:
        raise RuntimeError("context-cache live evidence is missing or oversized")
    observation = json.loads(args.evidence.read_text(encoding="utf-8"))
    report = context_cache_retirement.build_report(
        observation, now=datetime.now(timezone.utc), max_age_hours=args.max_age_hours)
    _atomic(args.output, report)
    print(json.dumps({"family": "context_cache", "status": "passed", "report": str(args.output)}))
    return 0


if __name__ == "__main__": raise SystemExit(main())
