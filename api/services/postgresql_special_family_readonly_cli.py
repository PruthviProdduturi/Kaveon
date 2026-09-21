"""Verify live special-family preservation without fencing or deleting rows."""

import argparse
import json
from pathlib import Path

from services import adls_artifact_client
from services import postgresql_baseline_identity
from services import postgresql_special_family_readonly as verifier


def _load(path: Path) -> dict:
    if not path.is_file() or path.stat().st_size > postgresql_baseline_identity.MAX_PAYLOAD_BYTES:
        raise RuntimeError(f"missing or oversized input: {path.name}")
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise RuntimeError(f"invalid input: {path.name}")
    return value


def main(argv=None, *, client_factory=adls_artifact_client.AzureArtifactClient.from_env):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", required=True, type=Path)
    parser.add_argument("--migration-evidence", required=True, type=Path)
    parser.add_argument("--prefix", required=True)
    parser.add_argument("--output-directory", required=True, type=Path)
    args = parser.parse_args(argv)
    return verifier.run(baseline=_load(args.baseline),
        migration_evidence=_load(args.migration_evidence), prefix=args.prefix,
        output_directory=args.output_directory, client=client_factory())


if __name__ == "__main__":
    try:
        print(json.dumps(main(), sort_keys=True, separators=(",", ":")))
    except Exception as error:
        print(json.dumps({"passed": False, "error": str(error)}, sort_keys=True,
                         separators=(",", ":")))
        raise SystemExit(1)
