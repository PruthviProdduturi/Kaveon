"""CLI for the workload-identity special-family ADLS publisher."""

import argparse
import json
import os
import tempfile
from pathlib import Path

from services import adls_artifact_client as adls
from services import postgresql_baseline_identity as baseline_identity
from services import postgresql_special_family_adls_publisher as adapter
from services import postgresql_special_family_migration as migration


def _load(path):
    if not path.is_file() or path.stat().st_size > baseline_identity.MAX_PAYLOAD_BYTES:
        raise RuntimeError("canonical special-family baseline is missing or oversized")
    return json.loads(path.read_text(encoding="utf-8"))


def _write_new(path, value):
    path = path.resolve()
    if path.exists(): raise RuntimeError("refusing to overwrite special-family migration evidence")
    path.parent.mkdir(parents=True, exist_ok=True)
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":"),
                         ensure_ascii=False).encode("utf-8") + b"\n"
    temporary = None
    try:
        with tempfile.NamedTemporaryFile("wb", dir=path.parent, prefix=path.name + ".",
                                         delete=False) as handle:
            temporary = Path(handle.name); os.chmod(temporary, 0o600)
            handle.write(encoded); handle.flush(); os.fsync(handle.fileno())
        os.replace(temporary, path); temporary = None
    finally:
        if temporary and temporary.exists(): temporary.unlink()


def main(argv=None, *, client_factory=adls.AzureArtifactClient.from_env):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", required=True, type=Path)
    parser.add_argument("--prefix", required=True)
    parser.add_argument("--expected-head-etag", required=True,
                        help="Current quoted ADLS ETag, or 'absent' for the first commit")
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args(argv)
    if os.getenv("KAVEON_SPECIAL_FAMILY_MIGRATION_ENABLED") != "true":
        raise RuntimeError("special-family migration requires explicit enablement")
    payload = _load(args.baseline)
    evidence = migration.publish(payload, expected_head=args.expected_head_etag,
        publisher=adapter.Publisher(client_factory(), args.prefix))
    _write_new(args.output, evidence)
    return {"passed": True, "baseline_evidence_id": evidence["baseline_evidence_id"],
            "manifest_sha256": evidence["manifest"]["sha256"],
            "status": evidence["manifest"]["status"]}


if __name__ == "__main__":
    try: print(json.dumps(main(), sort_keys=True, separators=(",", ":")))
    except Exception as error:
        print(json.dumps({"passed": False, "error": str(error)}, sort_keys=True,
                         separators=(",", ":")))
        raise SystemExit(1)
