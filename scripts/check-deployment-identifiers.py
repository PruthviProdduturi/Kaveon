"""Check the Git index for private deployment identifiers without printing their values."""

import hashlib
from pathlib import Path
import re
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[1]
FINGERPRINTS = ROOT / "scripts" / "private-deployment-fingerprints.txt"
TOKEN = re.compile(rb"[\w.+%-]+@[\w.-]+|[a-z0-9][a-z0-9_.-]*", re.I)
GUID = rb"[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}"
RESOURCE = re.compile(rb"/subscriptions/(" + GUID + rb")/", re.I)
CONFIG = re.compile(
    rb"(?:azure[_-])?(?:subscription|tenant)[_-]?id[\"']?\s*[:=]\s*[\"']?(" + GUID + rb")"
    rb"|login\.microsoftonline\.com/(" + GUID + rb")/", re.I,
)


def main():
    private = set(FINGERPRINTS.read_text(encoding="ascii").split())
    entries = subprocess.check_output(["git", "ls-files", "--stage", "-z"], cwd=ROOT)
    failures = 0
    with subprocess.Popen(["git", "cat-file", "--batch"], cwd=ROOT,
                          stdin=subprocess.PIPE, stdout=subprocess.PIPE) as git:
        for entry in entries.split(b"\0"):
            if not entry:
                continue
            meta, path = entry.split(b"\t", 1)
            mode, oid, stage = meta.split()
            if stage != b"0":
                raise ValueError("Resolve Git index conflicts before scanning")
            if mode == b"160000":
                continue
            git.stdin.write(oid + b"\n")
            git.stdin.flush()
            size = int(git.stdout.readline().split()[2])
            data = git.stdout.read(size)
            git.stdout.read(1)
            if b"\0" in data[:4096]:
                continue
            # Split dotted names too: an identifier can be the first DNS label.
            seen = set(TOKEN.findall(data.lower()))
            candidates = seen | {part for token in seen for part in token.split(b".")}
            candidates.update(re.findall(GUID, data.lower()))
            literal_ids = RESOURCE.findall(data)
            literal_ids.extend(a or b for a, b in CONFIG.findall(data))
            # Examples such as 11111111-1111-4111-8111-111111111111 have a repeated prefix and tail.
            configured = any(len(set(value[:8] + value[-12:])) > 1 for value in literal_ids)
            if configured or any(hashlib.sha256(x).hexdigest() in private for x in candidates):
                print("Private deployment identifier in", path.decode(errors="replace"))
                failures += 1
        git.stdin.close()
    return int(bool(failures))


if __name__ == "__main__":
    sys.exit(main())
