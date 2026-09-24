"""Run the one-shot control-plane replay inside the API image.

This is intentionally a separate command from the web process so a migration
can be observed and retried without enabling KaveonDB read authority first.
"""

import argparse
import json
from pathlib import Path
import tempfile

import database.metadata as db
from services import system_authority_replay


def main(argv=None) -> dict:
    parser = argparse.ArgumentParser(description="Replay PostgreSQL control-plane rows into KaveonDB")
    parser.add_argument("--family", choices=sorted(system_authority_replay.FAMILY_TABLES), action="append")
    parser.add_argument("--table", choices=sorted(system_authority_replay._TABLE_TO_FAMILY), action="append")
    parser.add_argument("--report", type=Path, help="write a JSON report atomically")
    args = parser.parse_args(argv)
    available = {
        row["table_name"]
        for row in db.query(
            "SELECT table_name FROM information_schema.tables "
            "WHERE table_schema = 'public'"
        )["rows"]
        if isinstance(row, dict) and isinstance(row.get("table_name"), str)
    }
    selected = set(args.family or system_authority_replay.FAMILY_TABLES)
    selected_tables = set(args.table or [])
    reports = []
    for family, tables in system_authority_replay.FAMILY_TABLES.items():
        if family not in selected:
            continue
        table_reports = []
        for table in tables:
            if selected_tables and table not in selected_tables:
                continue
            if table not in available:
                table_reports.append({
                    "family": family, "table": table, "source_count": 0,
                    "written": 0, "missing_source_table": True,
                })
                continue
            table_reports.append(system_authority_replay.replay_table(table))
        reports.append({
            "family": family,
            "tables": table_reports,
            "source_count": sum(int(item["source_count"]) for item in table_reports),
            "written": sum(int(item["written"]) for item in table_reports),
            "missing_source_tables": [item["table"] for item in table_reports if item.get("missing_source_table")],
        })
    result = {"families": reports}
    encoded = json.dumps(result, sort_keys=True, indent=2)
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.NamedTemporaryFile("w", encoding="utf-8", dir=args.report.parent,
                                         prefix=args.report.name + ".", delete=False) as handle:
            handle.write(encoded + "\n")
            temporary = Path(handle.name)
        temporary.replace(args.report)
    print(encoded)
    return result


if __name__ == "__main__":
    main()
