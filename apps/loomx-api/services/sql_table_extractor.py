"""
Extract table names referenced in a SQL query.
Port of sqlTableExtractor.ts — identical regex logic.
"""

import re
from typing import List


def extract_tables_from_sql(sql: str) -> List[str]:
    """
    Parse every FROM and JOIN clause to collect schema.table references.
    Returns a sorted, deduplicated list of normalised "schema.table" strings.
    """
    tables = set()

    # Strip single-line and multi-line SQL comments, then collapse whitespace
    stripped = re.sub(r"--[^\n]*", " ", sql)
    stripped = re.sub(r"/\*[\s\S]*?\*/", " ", stripped)
    stripped = re.sub(r"\s+", " ", stripped).strip()

    pattern = re.compile(
        r"\b(?:FROM|(?:INNER\s+|LEFT\s+|RIGHT\s+|FULL\s+|CROSS\s+|OUTER\s+)?JOIN)"
        r"\s+(?!\()(\[?[\w$]+\]?(?:\s*\.\s*\[?[\w$]+\]?)?)",
        re.IGNORECASE,
    )

    _SKIP = {"select", "where", "on", "and", "or", "set", "values",
             "openquery", "opendatasource", "openrowset"}

    for match in pattern.finditer(stripped):
        raw = match.group(1)
        raw = re.sub(r"[\[\]]", "", raw)   # strip brackets
        raw = re.sub(r"\s+", "", raw)       # remove whitespace around dot
        raw = raw.strip().lower()

        if (
            raw
            and len(raw) >= 1
            and not raw.startswith("#")
            and not raw.startswith("@")
            and raw not in _SKIP
        ):
            tables.add(raw)

    return sorted(tables)
