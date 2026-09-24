"""Run the one-shot control-plane replay inside the API image.

This is intentionally a separate command from the web process so a migration
can be observed and retried without enabling KaveonDB read authority first.
"""

import json

from services import system_authority_replay


def main() -> None:
    reports = [
        system_authority_replay.replay_family(family)
        for family in system_authority_replay.FAMILY_TABLES
    ]
    print(json.dumps({"families": reports}, sort_keys=True))


if __name__ == "__main__":
    main()
