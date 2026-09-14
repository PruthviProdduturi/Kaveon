"""Execute bounded live probes and sign PostgreSQL retirement observations."""

import argparse
import json
import os
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "api"))
from services import postgresql_operational_evidence as evidence  # noqa: E402


MAX_MANIFEST_BYTES = 256 * 1024
MAX_PROBE_OUTPUT_BYTES = 256 * 1024
MAX_ARG_COUNT = 128
MAX_ARG_BYTES = 16 * 1024
MANIFEST_KEYS = frozenset(("schema_version", "run_id", "probes"))
PROBE_KEYS = frozenset(("argv", "timeout_seconds"))


def _load_manifest(path):
    if not path.is_file() or path.stat().st_size > MAX_MANIFEST_BYTES:
        raise RuntimeError("probe manifest is missing or oversized")
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        raise RuntimeError("probe manifest is invalid") from error
    if not isinstance(value, dict) or set(value) != MANIFEST_KEYS or value["schema_version"] != 1:
        raise RuntimeError("probe manifest schema is invalid")
    if not isinstance(value["run_id"], str) or not 1 <= len(value["run_id"]) <= 128:
        raise RuntimeError("probe run ID is invalid")
    probes = value["probes"]
    if not isinstance(probes, dict) or set(probes) != set(evidence.GATES):
        raise RuntimeError("probe manifest must contain every operational gate exactly once")
    for gate, probe in probes.items():
        if not isinstance(probe, dict) or set(probe) != PROBE_KEYS:
            raise RuntimeError(f"probe configuration is invalid for {gate}")
        argv = probe["argv"]
        timeout = probe["timeout_seconds"]
        if (not isinstance(argv, list) or not argv or len(argv) > MAX_ARG_COUNT or
                any(not isinstance(arg, str) or not arg or len(arg.encode()) > MAX_ARG_BYTES for arg in argv)):
            raise RuntimeError(f"probe argv is invalid for {gate}")
        if type(timeout) is not int or not 1 <= timeout <= 1800:
            raise RuntimeError(f"probe timeout is invalid for {gate}")
    return value


def _run_probe(gate, probe, *, runner=subprocess.run):
    try:
        result = runner(probe["argv"], shell=False, capture_output=True,
                        timeout=probe["timeout_seconds"], check=False)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise RuntimeError(f"live operational probe could not complete for {gate}") from error
    stdout = result.stdout if isinstance(result.stdout, bytes) else str(result.stdout).encode()
    stderr = result.stderr if isinstance(result.stderr, bytes) else str(result.stderr).encode()
    if len(stdout) > MAX_PROBE_OUTPUT_BYTES or len(stderr) > MAX_PROBE_OUTPUT_BYTES:
        raise RuntimeError(f"live operational probe output is oversized for {gate}")
    if result.returncode != 0:
        raise RuntimeError(f"live operational probe failed for {gate} with exit code {result.returncode}")
    try:
        observation = json.loads(stdout.decode("utf-8"))
    except (UnicodeDecodeError, ValueError) as error:
        raise RuntimeError(f"live operational probe returned invalid JSON for {gate}") from error
    if not isinstance(observation, dict):
        raise RuntimeError(f"live operational probe returned a non-object for {gate}")
    return observation


def record(manifest, *, checked_at, max_rollback_seconds=900, runner=subprocess.run):
    receipts = {}
    for gate in evidence.GATES:
        observation = _run_probe(gate, manifest["probes"][gate], runner=runner)
        receipts[gate] = evidence.receipt_from_observation(
            gate, observation, checked_at=checked_at,
            evidence_id=f"{manifest['run_id']}:{gate}",
            max_rollback_seconds=max_rollback_seconds)
    return receipts


def _publish(directory, receipts):
    directory.mkdir(parents=True, exist_ok=True)
    for gate, receipt in receipts.items():
        destination = directory / f"{gate}.json"
        temporary = directory / f".{gate}.{os.getpid()}.tmp"
        temporary.write_text(json.dumps(receipt, sort_keys=True, separators=(",", ":")) + "\n",
                             encoding="utf-8")
        os.replace(temporary, destination)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--max-rollback-seconds", type=int, default=900)
    args = parser.parse_args()
    try:
        manifest = _load_manifest(args.manifest)
        checked_at = datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")
        receipts = record(manifest, checked_at=checked_at,
                          max_rollback_seconds=args.max_rollback_seconds)
        _publish(args.output, receipts)
        print(json.dumps({"passed": True, "receipt_count": len(receipts),
                          "run_id": manifest["run_id"]}, sort_keys=True))
        return 0
    except (OSError, RuntimeError, ValueError) as error:
        print(json.dumps({"passed": False, "error": str(error)}, sort_keys=True))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
