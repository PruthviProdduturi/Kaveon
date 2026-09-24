"""Run the one-shot control-plane replay inside the API image.

This is intentionally a separate command from the web process so a migration
can be observed and retried without enabling KaveonDB read authority first.
"""

import json

import database.metadata as db
from services import system_authority_replay


def main() -> None:
    available = {
        row["table_name"]
        for row in db.query(
            "SELECT table_name FROM information_schema.tables "
            "WHERE table_schema = 'public'"
        )["rows"]
        if isinstance(row, dict) and isinstance(row.get("table_name"), str)
    }
    reports = []
    for family, tables in system_authority_replay.FAMILY_TABLES.items():
        table_reports = []
        for table in tables:
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
    print(json.dumps({"families": reports}, sort_keys=True))


if __name__ == "__main__":
    main()
