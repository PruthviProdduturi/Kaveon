"""Validate the authoritative bounded T-SQL compatibility corpus and tracker.

This is intentionally small and dependency-free so release checks can run it
before Rust compilation. It catches the bookkeeping failures that otherwise
make a compatibility claim stale: duplicate case ids, missing statuses,
corpus/tracker count drift, and stale legacy corpus wording.
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
CORPUS = ROOT / "docs" / "qualification" / "tsql-read-compatibility-corpus-2026-10-28.json"
TRACKER = ROOT / "docs" / "engineering" / "trino-parity-tracker.md"
QUALIFICATION_DIR = ROOT / "docs" / "qualification"
COUNTED_DOCUMENTS = (
    TRACKER,
    ROOT / "docs" / "engineering" / "sql-compatibility-gap-plan.md",
)


def fail(message: str) -> None:
    raise SystemExit(f"FAIL: {message}")


def main() -> int:
    try:
        corpus = json.loads(CORPUS.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot read compatibility corpus: {error}")

    cases = corpus.get("cases")
    if not isinstance(cases, list) or not cases:
        fail("corpus must contain a non-empty cases array")

    allowed_statuses = {"planner_qualified", "explicit_rejection"}
    ids: list[str] = []
    for index, case in enumerate(cases):
        if not isinstance(case, dict):
            fail(f"case {index} is not an object")
        case_id = case.get("id")
        sql = case.get("sql")
        status = case.get("status")
        if not isinstance(case_id, str) or not case_id:
            fail(f"case {index} has no non-empty id")
        if not isinstance(sql, str) or not sql.strip():
            fail(f"case {case_id} has no SQL text")
        if status not in allowed_statuses:
            fail(f"case {case_id} has unsupported status {status!r}")
        if "distributed_status" in case and not isinstance(case["distributed_status"], str):
            fail(f"case {case_id} has a non-string distributed_status")
        ids.append(case_id)

    duplicates = sorted({case_id for case_id in ids if ids.count(case_id) > 1})
    if duplicates:
        fail(f"duplicate case ids: {', '.join(duplicates)}")

    engine_commit = corpus.get("engine_commit")
    if not isinstance(engine_commit, str) or not re.fullmatch(r"[0-9a-f]{7,40}", engine_commit):
        fail("corpus engine_commit must be a lower-case hexadecimal Git revision")
    try:
        resolved = subprocess.run(
            ["git", "cat-file", "-e", f"{engine_commit}^{{commit}}"],
            cwd=ROOT,
            check=False,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    except OSError as error:
        fail(f"cannot validate corpus engine_commit: {error}")
    if resolved.returncode != 0:
        fail(f"corpus engine_commit is not present in this repository: {engine_commit}")

    try:
        tracker = TRACKER.read_text(encoding="utf-8")
    except OSError as error:
        fail(f"cannot read parity tracker: {error}")

    count_matches = re.findall(r"corpus currently contains (\d+) bounded T-SQL read cases", tracker)
    if not count_matches:
        fail("tracker is missing its authoritative corpus count")
    if any(int(value) != len(cases) for value in count_matches):
        fail(f"tracker corpus count {count_matches!r} does not match {len(cases)} cases")
    stale_count_pattern = re.compile(
        r"\b(?:current|complete|versioned)\s+\d+(?:-case\s+versioned\s+corpus|-case|\s+cases)\b",
        re.IGNORECASE,
    )
    authoritative_count_patterns = (
        re.compile(
            r"authoritative\s+(?:current\s+)?(?:T-SQL\s+)?corpus"
            r"(?:\s+now)?\s+contains\s+(\d+)\s+cases",
            re.IGNORECASE,
        ),
        re.compile(r"authoritative\s+current\s+corpus\s+is\s+(\d+)\s+cases", re.IGNORECASE),
        re.compile(r"authoritative\s+current\s+total\s+is\s+the\s+(\d+)-case\s+count", re.IGNORECASE),
        re.compile(r"current\s+T-SQL\s+corpus\s+is\s+(\d+)\s+cases", re.IGNORECASE),
        re.compile(r"versioned\s+(\d+)-case\s+corpus", re.IGNORECASE),
    )
    for document in COUNTED_DOCUMENTS:
        try:
            text = document.read_text(encoding="utf-8", errors="replace")
        except OSError as error:
            fail(f"cannot read counted compatibility document {document}: {error}")
        for match in stale_count_pattern.finditer(text):
            number = int(re.search(r"\d+", match.group(0)).group(0))
            if number != len(cases):
                fail(
                    f"stale compatibility count in {document.relative_to(ROOT)}: "
                    f"{match.group(0)!r}; expected {len(cases)}"
                )
        for pattern in authoritative_count_patterns:
            for match in pattern.finditer(text):
                number = int(match.group(1))
                if number != len(cases):
                    fail(
                        f"stale authoritative compatibility count in {document.relative_to(ROOT)}: "
                        f"{match.group(0)!r}; expected {len(cases)}"
                    )
    # Qualification notes retain historical run sizes, but must never call an
    # older count authoritative. Scan all notes for only the explicit current/
    # authoritative forms; historical wording such as "140-case historical
    # corpus revision" is intentionally allowed.
    for document in sorted(QUALIFICATION_DIR.glob("**/*.md")):
        try:
            text = document.read_text(encoding="utf-8", errors="replace")
        except OSError as error:
            fail(f"cannot read qualification document {document.relative_to(ROOT)}: {error}")
        for pattern in authoritative_count_patterns:
            for match in pattern.finditer(text):
                number = int(match.group(1))
                if number != len(cases):
                    fail(
                        f"stale authoritative compatibility count in {document.relative_to(ROOT)}: "
                        f"{match.group(0)!r}; expected {len(cases)}"
                    )
    if "tsql-read-compatibility-corpus-2026-10-28.json" not in tracker:
        fail("tracker does not link the authoritative corpus")
    pin_matches = re.findall(r"corpus pinned at\s*`([0-9a-f]{7,40})`", tracker)
    if not pin_matches:
        fail("tracker is missing its authoritative engine commit pin")
    if any(pin != engine_commit for pin in pin_matches):
        fail(f"tracker engine pin {pin_matches!r} does not match corpus pin {engine_commit}")

    print(
        f"PASS: {len(cases)} unique T-SQL compatibility cases, "
        f"engine pin {engine_commit}, tracker count synchronized"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
